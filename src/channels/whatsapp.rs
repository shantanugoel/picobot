use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use qrcode::QrCode;
use qrcode::render::unicode;
use tokio::sync::{mpsc, watch};
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::channels::permissions::channel_profile;
use crate::config::{Config, WhatsappConfig};
use crate::kernel::core::Kernel;
use crate::providers::factory::{ProviderAgent, ProviderAgentBuilder, ProviderFactory};
use crate::session::manager::SessionManager;
use crate::session::memory::MemoryRetriever;
use crate::session::types::{MessageType, StoredMessage};

#[async_trait]
pub trait WhatsAppBackend: Send + Sync {
    async fn start(&self) -> Result<()>;
    async fn send_text(&self, to: &str, body: &str) -> Result<String>;
    fn inbound_stream(&self) -> Pin<Box<dyn Stream<Item = InboundMessage> + Send>>;
}

pub struct WhatsappRustBackend {
    inbound_rx: Mutex<Option<mpsc::UnboundedReceiver<InboundMessage>>>,
    outbound_tx: mpsc::UnboundedSender<WhatsappOutbound>,
}

struct WhatsappOutbound {
    to: String,
    text: String,
    reply: tokio::sync::oneshot::Sender<Result<String>>,
}

impl WhatsappRustBackend {
    pub fn new(store_path: String, qr_cache: watch::Sender<Option<String>>) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_whatsapp_loop(
            store_path,
            inbound_tx,
            outbound_rx,
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
    async fn start(&self) -> Result<()> {
        Ok(())
    }

    async fn send_text(&self, to: &str, body: &str) -> Result<String> {
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

#[derive(Debug, Clone)]
pub struct InboundMessage {
    pub channel_id: String,
    pub user_id: String,
    pub text: String,
    pub message_id: Option<String>,
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

    pub async fn subscribe(&self) -> Pin<Box<dyn Stream<Item = InboundMessage> + Send>> {
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

pub struct WhatsAppOutboundSender {
    backend: Arc<dyn WhatsAppBackend>,
}

impl WhatsAppOutboundSender {
    pub fn new(backend: Arc<dyn WhatsAppBackend>) -> Self {
        Self { backend }
    }

    pub async fn send(&self, user_id: &str, text: &str) -> Result<String> {
        match self.backend.send_text(user_id, text).await {
            Ok(delivery_id) => Ok(delivery_id),
            Err(err) => {
                eprintln!("WhatsApp send failed to '{user_id}': {err}");
                Err(err)
            }
        }
    }
}

pub async fn run(
    config: Config,
    kernel: Kernel,
    agent_builder: ProviderAgentBuilder,
) -> Result<()> {
    let whatsapp_config = config.whatsapp();
    if whatsapp_config.enabled == Some(false) {
        println!("WhatsApp channel disabled via config");
        return Ok(());
    }

    let store_path = whatsapp_store_path(&config, &whatsapp_config);
    let allowed_senders = whatsapp_allowed_senders(&whatsapp_config);

    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let profile = channel_profile(&config.channels(), "whatsapp", &base_dir);
    let base_kernel = kernel
        .with_prompt_profile(profile)
        .with_channel_id(Some("whatsapp".to_string()));

    let session_store = crate::session::db::SqliteStore::new(
        config
            .data_dir()
            .join("sessions.db")
            .to_string_lossy()
            .to_string(),
    );
    session_store.touch()?;
    let memory_config = config.memory();
    let session_manager = SessionManager::new(session_store.clone());
    let memory_retriever = MemoryRetriever::new(memory_config.clone(), session_store);
    let agent_router = ProviderFactory::build_agent_router(&config)
        .ok()
        .filter(|router| !router.is_empty());

    let (qr_cache_tx, mut qr_cache_rx) = watch::channel(None);
    let backend: Arc<dyn WhatsAppBackend> =
        Arc::new(WhatsappRustBackend::new(store_path, qr_cache_tx));
    tokio::spawn(async move {
        while qr_cache_rx.changed().await.is_ok() {
            if let Some(code) = qr_cache_rx.borrow().clone() {
                println!("WhatsApp QR Code:\n{}", render_qr_code(&code));
            }
        }
    });

    let inbound = WhatsAppInboundAdapter::new(Arc::clone(&backend), allowed_senders);
    let outbound = WhatsAppOutboundSender::new(Arc::clone(&backend));
    backend.start().await?;

    let mut inbound_stream = inbound.subscribe().await;
    while let Some(message) = inbound_stream.next().await {
        let user_id = message.user_id.clone();
        let session_id = format!("whatsapp:{user_id}");
        let session = match session_manager.get_session(&session_id)? {
            Some(session) => session,
            None => session_manager.create_session(
                session_id,
                "whatsapp".to_string(),
                "whatsapp".to_string(),
                user_id.clone(),
                base_kernel.context().capabilities.as_ref().clone(),
            )?,
        };

        let existing_messages = session_manager
            .get_messages(
                &session.id,
                memory_config.max_session_messages.unwrap_or(50),
            )
            .unwrap_or_default();
        let filtered_messages = if memory_config.include_tool_messages() {
            existing_messages
        } else {
            existing_messages
                .into_iter()
                .filter(|message| message.message_type != MessageType::Tool)
                .collect::<Vec<_>>()
        };
        let context_messages =
            memory_retriever.build_context(Some(&user_id), Some(&session.id), &filtered_messages);
        let context_snippet = MemoryRetriever::to_prompt_snippet(&context_messages);
        let prompt_to_send = if let Some(context) = context_snippet {
            format!("Context:\n{context}\n\nUser: {}", message.text)
        } else {
            message.text.clone()
        };

        let mut seq_order = match session_manager.get_messages(&session.id, 1) {
            Ok(messages) => messages
                .last()
                .map(|message| message.seq_order + 1)
                .unwrap_or(0),
            Err(_) => 0,
        };

        let user_message = StoredMessage {
            message_type: MessageType::User,
            content: message.text.clone(),
            tool_call_id: None,
            seq_order,
            token_estimate: None,
        };
        if session_manager
            .append_message(&session.id, &user_message)
            .is_ok()
        {
            seq_order += 1;
        }

        let message_kernel = Arc::new(
            base_kernel.clone_with_context(Some(user_id.clone()), Some(session.id.clone())),
        );
        let agent = build_agent_for_kernel(
            &config,
            &agent_builder,
            agent_router.as_ref(),
            message_kernel,
        )?;
        let response = prompt_with_agent(&agent, &prompt_to_send, config.max_turns())
            .await
            .unwrap_or_else(|err| format!("Sorry, something went wrong: {err}"));
        let assistant_message = StoredMessage {
            message_type: MessageType::Assistant,
            content: response.clone(),
            tool_call_id: None,
            seq_order,
            token_estimate: None,
        };
        let _ = session_manager.append_message(&session.id, &assistant_message);
        let _ = session_manager.touch(&session.id);

        let _ = outbound.send(&user_id, &response).await;
    }

    Ok(())
}

fn build_agent_for_kernel(
    config: &Config,
    agent_builder: &ProviderAgentBuilder,
    agent_router: Option<&crate::providers::factory::ModelRouter>,
    kernel: Arc<Kernel>,
) -> Result<ProviderAgent> {
    let registry = kernel.tool_registry();
    let kernel_clone = Arc::clone(&kernel);
    if let Some(router) = agent_router {
        router.build_default(config, registry, kernel_clone, config.max_turns())
    } else {
        Ok(agent_builder
            .clone()
            .build(registry, kernel_clone, config.max_turns()))
    }
}

fn whatsapp_store_path(config: &Config, channel: &WhatsappConfig) -> String {
    if let Some(path) = &channel.store_path {
        return path.to_string();
    }
    config
        .data_dir()
        .join("whatsapp.db")
        .to_string_lossy()
        .to_string()
}

fn whatsapp_allowed_senders(channel: &WhatsappConfig) -> Option<Vec<String>> {
    channel.allowed_senders.as_ref().and_then(|list| {
        if list.is_empty() {
            None
        } else {
            Some(list.clone())
        }
    })
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

async fn prompt_with_agent(
    agent: &ProviderAgent,
    prompt: &str,
    max_turns: usize,
) -> Result<String> {
    let response = agent
        .prompt_with_turns(prompt.to_string(), max_turns)
        .await
        .context("prompt failed")?;
    Ok(response)
}

async fn run_whatsapp_loop(
    store_path: String,
    inbound_tx: mpsc::UnboundedSender<InboundMessage>,
    mut outbound_rx: mpsc::UnboundedReceiver<WhatsappOutbound>,
    qr_cache: watch::Sender<Option<String>>,
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
            let qr_cache = qr_cache.clone();
            let client_tx = client_tx.clone();
            async move {
                let _ = client_tx.send(StdArc::clone(&client));
                match event {
                    Event::PairingQrCode { code, .. } => {
                        let _ = qr_cache.send(Some(code));
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
                            message_id: Some(info.id.to_string()),
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
) -> Result<String> {
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
