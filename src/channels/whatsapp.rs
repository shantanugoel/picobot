use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::Context;
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use qrcode::QrCode;
use qrcode::render::unicode;
use tokio::sync::{broadcast, mpsc, watch};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::channels::adapter::{
    ChannelType, DeliveryId, InboundAdapter, InboundMessage, OutboundMessage, OutboundSender,
};

#[async_trait]
pub trait WhatsAppBackend: Send + Sync {
    async fn start(&self) -> Result<(), anyhow::Error>;
    async fn send_text(&self, to: &str, body: &str) -> Result<DeliveryId, anyhow::Error>;
    fn inbound_stream(&self) -> Pin<Box<dyn Stream<Item = InboundMessage> + Send>>;
}

pub struct WhatsappRustBackend {
    inbound_rx: Mutex<Option<mpsc::UnboundedReceiver<InboundMessage>>>,
    outbound_tx: mpsc::UnboundedSender<WhatsappOutbound>,
}

struct WhatsappOutbound {
    to: String,
    text: String,
    reply: tokio::sync::oneshot::Sender<Result<DeliveryId, anyhow::Error>>,
}

impl WhatsappRustBackend {
    pub fn new(
        store_path: String,
        qr_tx: Option<broadcast::Sender<String>>,
        qr_cache: Option<watch::Sender<Option<String>>>,
    ) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_whatsapp_loop(
            store_path,
            inbound_tx,
            outbound_rx,
            qr_tx.clone(),
            qr_cache,
        ));
        Self {
            inbound_rx: Mutex::new(Some(inbound_rx)),
            outbound_tx,
        }
    }
}

#[async_trait]
impl WhatsAppBackend for WhatsappRustBackend {
    async fn start(&self) -> Result<(), anyhow::Error> {
        Ok(())
    }

    async fn send_text(&self, to: &str, body: &str) -> Result<DeliveryId, anyhow::Error> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.outbound_tx
            .send(WhatsappOutbound {
                to: to.to_string(),
                text: body.to_string(),
                reply: tx,
            })
            .context("whatsapp outbound channel closed")?;
        rx.await.context("whatsapp outbound response closed")?
    }

    fn inbound_stream(&self) -> Pin<Box<dyn Stream<Item = InboundMessage> + Send>> {
        let mut guard = self
            .inbound_rx
            .lock()
            .expect("inbound stream mutex poisoned");
        let receiver = guard.take().expect("inbound stream already taken");
        Box::pin(UnboundedReceiverStream::new(receiver))
    }
}

pub struct WhatsAppInboundAdapter {
    backend: Arc<dyn WhatsAppBackend>,
    allowed_senders: Option<Vec<String>>,
}

impl WhatsAppInboundAdapter {
    pub fn new(backend: Arc<dyn WhatsAppBackend>, allowed_senders: Option<Vec<String>>) -> Self {
        Self {
            backend,
            allowed_senders,
        }
    }
}

#[async_trait]
impl InboundAdapter for WhatsAppInboundAdapter {
    fn adapter_id(&self) -> &str {
        "whatsapp"
    }

    fn channel_type(&self) -> ChannelType {
        ChannelType::Whatsapp
    }

    async fn subscribe(&self) -> Pin<Box<dyn Stream<Item = InboundMessage> + Send>> {
        if let Some(allowed) = self.allowed_senders.clone() {
            let stream = self.backend.inbound_stream().filter_map(move |message| {
                let allowed = allowed.clone();
                async move {
                    let user = message.user_id.clone();
                    if is_allowed_sender(&user, &allowed) {
                        println!("WhatsApp received message from '{user}'");
                        Some(message)
                    } else {
                        println!("WhatsApp ignored message from '{user}' (not in allowlist)");
                        None
                    }
                }
            });
            return Box::pin(stream);
        }
        self.backend.inbound_stream()
    }
}

fn is_allowed_sender(sender: &str, allowed: &[String]) -> bool {
    if allowed
        .iter()
        .any(|allowed_sender| allowed_sender == sender)
    {
        return true;
    }
    let sender_id = normalize_whatsapp_id(sender);
    allowed
        .iter()
        .any(|allowed_sender| normalize_whatsapp_id(allowed_sender) == sender_id)
}

fn normalize_whatsapp_id(sender: &str) -> &str {
    sender.split_once('@').map(|(id, _)| id).unwrap_or(sender)
}

pub struct WhatsAppOutboundSender {
    backend: Arc<dyn WhatsAppBackend>,
}

impl WhatsAppOutboundSender {
    pub fn new(backend: Arc<dyn WhatsAppBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl OutboundSender for WhatsAppOutboundSender {
    fn sender_id(&self) -> &str {
        "whatsapp"
    }

    fn supports_streaming(&self) -> bool {
        false
    }

    async fn send(&self, msg: OutboundMessage) -> Result<DeliveryId, anyhow::Error> {
        match self.backend.send_text(&msg.user_id, &msg.text).await {
            Ok(delivery_id) => Ok(delivery_id),
            Err(err) => {
                eprintln!("WhatsApp send failed to '{}': {err}", msg.user_id);
                Err(err)
            }
        }
    }

    async fn stream_token(&self, _session_id: &str, _token: &str) -> Result<(), anyhow::Error> {
        Ok(())
    }
}

async fn run_whatsapp_loop(
    store_path: String,
    inbound_tx: mpsc::UnboundedSender<InboundMessage>,
    mut outbound_rx: mpsc::UnboundedReceiver<WhatsappOutbound>,
    qr_tx: Option<broadcast::Sender<String>>,
    qr_cache: Option<watch::Sender<Option<String>>>,
) {
    use std::sync::Arc as StdArc;

    use wacore::types::events::Event;
    use whatsapp_rust::bot::Bot;
    use whatsapp_rust_sqlite_storage::SqliteStore;
    use whatsapp_rust_tokio_transport::TokioWebSocketTransportFactory;
    use whatsapp_rust_ureq_http_client::UreqHttpClient;

    let backend = match SqliteStore::new(&store_path).await {
        Ok(store) => StdArc::new(store),
        Err(err) => {
            eprintln!("WhatsApp store init failed: {err}");
            return;
        }
    };

    let (client_tx, mut client_rx) = mpsc::unbounded_channel();

    let mut bot = match Bot::builder()
        .with_backend(backend)
        .with_transport_factory(TokioWebSocketTransportFactory::new())
        .with_http_client(UreqHttpClient::new())
        .on_event(move |event, client| {
            let inbound_tx = inbound_tx.clone();
            let qr_tx = qr_tx.clone();
            let qr_cache = qr_cache.clone();
            let client_tx = client_tx.clone();
            async move {
                let _ = client_tx.send(StdArc::clone(&client));
                match event {
                    Event::PairingQrCode { code, .. } => {
                        if let Some(cache) = qr_cache.clone() {
                            let _ = cache.send(Some(code.clone()));
                        }
                        if let Some(tx) = qr_tx {
                            let delivered = tx.send(code.clone()).unwrap_or(0);
                            if delivered == 0 {
                                println!("WhatsApp QR Code:\n{}", render_qr_code(&code));
                            }
                        } else {
                            println!("WhatsApp QR Code:\n{}", render_qr_code(&code));
                        }
                    }
                    Event::Message(message, info) => {
                        let text = message
                            .conversation
                            .clone()
                            .or_else(|| {
                                message
                                    .extended_text_message
                                    .as_ref()
                                    .and_then(|ext| ext.text.clone())
                            })
                            .unwrap_or_default();
                        if text.trim().is_empty() {
                            return;
                        }
                        let from = info.source.sender.to_string();
                        let _ = inbound_tx.send(InboundMessage {
                            channel_id: "whatsapp".to_string(),
                            user_id: from,
                            text,
                        });
                    }
                    _ => {}
                }
            }
        })
        .build()
        .await
    {
        Ok(bot) => bot,
        Err(err) => {
            eprintln!("WhatsApp bot build failed: {err}");
            return;
        }
    };

    let mut run_task = tokio::spawn(async move {
        match bot.run().await {
            Ok(handle) => {
                if let Err(err) = handle.await {
                    eprintln!("WhatsApp bot task error: {err}");
                }
            }
            Err(err) => {
                eprintln!("WhatsApp bot error: {err}");
            }
        }
    });

    let client = tokio::select! {
        Some(client) = client_rx.recv() => client,
        _ = &mut run_task => return,
    };

    while let Some(command) = outbound_rx.recv().await {
        let reply = send_outbound_message(&client, &command.to, &command.text).await;
        let _ = command.reply.send(reply);
    }
}

async fn send_outbound_message(
    client: &Arc<whatsapp_rust::Client>,
    to: &str,
    body: &str,
) -> Result<DeliveryId, anyhow::Error> {
    use wacore_binary::jid::Jid;
    use waproto::whatsapp as wa;

    let jid: Jid = to.parse().context("invalid whatsapp jid")?;
    let message = wa::Message {
        conversation: Some(body.to_string()),
        ..Default::default()
    };
    let message_id = client.send_message(jid, message).await?;
    Ok(message_id)
}

fn render_qr_code(payload: &str) -> String {
    let code = match QrCode::new(payload) {
        Ok(code) => code,
        Err(_) => return payload.to_string(),
    };
    code.render::<unicode::Dense1x2>().quiet_zone(true).build()
}
