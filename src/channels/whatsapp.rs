use std::pin::Pin;
use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use futures::Stream;
use tokio::sync::{Mutex, broadcast, mpsc};
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
    pub fn new(store_path: String, qr_tx: Option<broadcast::Sender<String>>) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_whatsapp_loop(
            store_path,
            inbound_tx,
            outbound_rx,
            qr_tx.clone(),
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
        let mut guard = self.inbound_rx.blocking_lock();
        let receiver = guard.take().expect("inbound stream already taken");
        Box::pin(UnboundedReceiverStream::new(receiver))
    }
}

pub struct WhatsAppInboundAdapter {
    backend: Arc<dyn WhatsAppBackend>,
}

impl WhatsAppInboundAdapter {
    pub fn new(backend: Arc<dyn WhatsAppBackend>) -> Self {
        Self { backend }
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
        self.backend.inbound_stream()
    }
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
        self.backend.send_text(&msg.user_id, &msg.text).await
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
            let client_tx = client_tx.clone();
            async move {
                let _ = client_tx.send(StdArc::clone(&client));
                match event {
                    Event::PairingQrCode { code, .. } => {
                        if let Some(tx) = qr_tx {
                            let _ = tx.send(code);
                        } else {
                            println!("WhatsApp QR Code:\n{code}");
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
