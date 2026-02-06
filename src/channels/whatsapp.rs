use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use async_trait::async_trait;
use dashmap::DashMap;
use futures::{Stream, StreamExt};
use qrcode::QrCode;
use qrcode::render::unicode;
use tokio::sync::{Mutex as AsyncMutex, Semaphore, mpsc, watch};
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;
use wacore::proto_helpers::MessageExt;

use crate::channels::permissions::channel_profile;
use crate::config::{Config, WhatsappConfig};
use crate::kernel::core::Kernel;
use crate::kernel::permissions::{PathPattern, Permission};
use crate::providers::factory::{
    DEFAULT_PROVIDER_RETRIES, ProviderAgent, ProviderAgentBuilder, ProviderFactory,
};
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
    pub fn new(
        store_path: String,
        media_root: PathBuf,
        max_media_size_bytes: u64,
        allowed_senders: Option<Vec<String>>,
        qr_cache: watch::Sender<Option<String>>,
    ) -> Self {
        let (inbound_tx, inbound_rx) = mpsc::unbounded_channel();
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
        tokio::spawn(run_whatsapp_loop(
            store_path,
            media_root,
            max_media_size_bytes,
            allowed_senders,
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
    pub attachments: Vec<MediaAttachment>,
}

#[derive(Debug, Clone)]
pub struct MediaAttachment {
    pub media_type: MediaType,
    pub mime_type: Option<String>,
    pub file_name: Option<String>,
    pub local_path: PathBuf,
    pub caption: Option<String>,
    pub size_bytes: Option<u64>,
    pub thumbnail_path: Option<PathBuf>,
    pub thumbnail_mime_type: Option<String>,
    pub thumbnail_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy)]
pub enum MediaType {
    Image,
    Document,
    Audio,
    Video,
    Sticker,
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
                        tracing::info!(user = %user, "WhatsApp received message");
                        Some(message)
                    } else {
                        tracing::info!(user = %user, "WhatsApp ignored message (not in allowlist)");
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
                tracing::error!(user = %user_id, error = %err, "WhatsApp send failed");
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
        tracing::info!("WhatsApp channel disabled via config");
        return Ok(());
    }

    let store_path = whatsapp_store_path(&config, &whatsapp_config);
    let allowed_senders = whatsapp_allowed_senders(&whatsapp_config);
    let media_root = whatsapp_media_root(&config, &whatsapp_config);

    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let mut profile = channel_profile(&config.channels(), "whatsapp", &base_dir);
    let media_perm = Permission::FileRead {
        path: PathPattern(format!("{}/**", media_root.display())),
    };
    profile.pre_authorized.insert(media_perm.clone());
    profile.max_allowed.insert(media_perm);
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

    ensure_media_dir(&media_root)?;
    let (qr_cache_tx, mut qr_cache_rx) = watch::channel(None);
    let backend: Arc<dyn WhatsAppBackend> = Arc::new(WhatsappRustBackend::new(
        store_path,
        media_root.clone(),
        whatsapp_config.max_media_size_bytes(),
        allowed_senders.clone(),
        qr_cache_tx,
    ));
    tokio::spawn(async move {
        while qr_cache_rx.changed().await.is_ok() {
            if let Some(code) = qr_cache_rx.borrow().clone() {
                tracing::info!("WhatsApp QR Code:\n{}", render_qr_code(&code));
            }
        }
    });

    let inbound = WhatsAppInboundAdapter::new(Arc::clone(&backend), allowed_senders);
    let outbound = Arc::new(WhatsAppOutboundSender::new(Arc::clone(&backend)));
    let mut base_kernel = base_kernel;
    if config.notifications().enabled() {
        let queue_config = crate::notifications::queue::NotificationQueueConfig {
            max_attempts: config.notifications().max_attempts(),
            base_backoff: Duration::from_millis(config.notifications().base_backoff_ms()),
            max_backoff: Duration::from_millis(config.notifications().max_backoff_ms()),
        };
        let queue = crate::notifications::queue::NotificationQueue::new(queue_config);
        let channel = Arc::new(
            crate::notifications::whatsapp::WhatsAppNotificationChannel::new(outbound.clone()),
        );
        let notifications = crate::notifications::service::NotificationService::new(queue, channel);
        let worker = notifications.clone();
        tokio::spawn(async move {
            worker.worker_loop().await;
        });
        let notification_arc = Arc::new(notifications);
        base_kernel = base_kernel.with_notifications(Some(notification_arc.clone()));
        if let Some(scheduler) = base_kernel.context().scheduler.clone() {
            scheduler.set_notifications(Some(notification_arc)).await;
        }
    }
    backend.start().await?;

    let max_concurrent = whatsapp_config.max_concurrent_messages();
    let global_semaphore = Arc::new(Semaphore::new(max_concurrent));
    let per_user_locks: Arc<DashMap<String, Arc<AsyncMutex<()>>>> = Arc::new(DashMap::new());

    let cleanup_root = media_root.clone();
    let retention_hours = whatsapp_config.media_retention_hours();
    tokio::spawn(async move {
        loop {
            cleanup_expired_media(&cleanup_root, retention_hours).await;
            tokio::time::sleep(Duration::from_secs(60 * 60)).await;
        }
    });

    let mut inbound_stream = inbound.subscribe().await;
    while let Some(message) = inbound_stream.next().await {
        let permit = match global_semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => continue,
        };
        let user_lock = per_user_locks
            .entry(message.user_id.clone())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone();
        let config = config.clone();
        let agent_builder = agent_builder.clone();
        let agent_router = agent_router.clone();
        let session_manager = session_manager.clone();
        let memory_retriever = memory_retriever.clone();
        let memory_config = memory_config.clone();
        let outbound = outbound.clone();
        let base_kernel = base_kernel.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _user_guard = user_lock.lock().await;
            let user_id = message.user_id.clone();
            let session_id = format!("whatsapp:{user_id}");
            let session = match session_manager.get_session(&session_id) {
                Ok(Some(session)) => session,
                Ok(None) => match session_manager.create_session(
                    session_id,
                    "whatsapp".to_string(),
                    "whatsapp".to_string(),
                    user_id.clone(),
                    base_kernel.context().capabilities.as_ref().clone(),
                ) {
                    Ok(session) => session,
                    Err(err) => {
                        let _ = outbound
                            .send(&user_id, &format!("Sorry, session error: {err}"))
                            .await;
                        return;
                    }
                },
                Err(err) => {
                    let _ = outbound
                        .send(&user_id, &format!("Sorry, session error: {err}"))
                        .await;
                    return;
                }
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
            let context_messages = memory_retriever.build_context(
                Some(&user_id),
                Some(&session.id),
                &filtered_messages,
            );
            let context_snippet = MemoryRetriever::to_prompt_snippet(&context_messages);
            let attachment_prompt = format_attachments_prompt(&message.attachments);
            let user_text = if attachment_prompt.is_empty() {
                message.text.clone()
            } else if message.text.trim().is_empty() {
                attachment_prompt
            } else {
                format!("{}\n\n{}", attachment_prompt, message.text)
            };
            let prompt_to_send = if let Some(context) = context_snippet {
                format!("Context:\n{context}\n\nUser: {user_text}")
            } else {
                user_text.clone()
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
                content: user_text.clone(),
                tool_call_id: None,
                seq_order,
                token_estimate: None,
            };
            match session_manager.append_message(&session.id, &user_message) {
                Ok(()) => seq_order += 1,
                Err(err) => {
                    tracing::warn!(error = %err, "failed to store user message");
                }
            }

            let message_kernel = Arc::new(
                base_kernel.clone_with_context(Some(user_id.clone()), Some(session.id.clone())),
            );
            let message_kernel = with_media_permissions(message_kernel, &message.attachments);
            let agent = match build_agent_for_kernel(
                &config,
                &agent_builder,
                agent_router.as_ref(),
                message_kernel,
            ) {
                Ok(agent) => agent,
                Err(err) => {
                    let _ = outbound
                        .send(&user_id, &format!("Sorry, agent error: {err}"))
                        .await;
                    return;
                }
            };
            let response =
                match prompt_with_agent(&agent, &prompt_to_send, config.max_turns()).await {
                    Ok(response) => response,
                    Err(err) => {
                        tracing::error!(error = %err, "prompt failed");
                        format!("Sorry, something went wrong: {err}")
                    }
                };
            let assistant_message = StoredMessage {
                message_type: MessageType::Assistant,
                content: response.clone(),
                tool_call_id: None,
                seq_order,
                token_estimate: None,
            };
            if let Err(err) = session_manager.append_message(&session.id, &assistant_message) {
                tracing::warn!(error = %err, "failed to store assistant message");
            }
            if let Err(err) = session_manager.touch(&session.id) {
                tracing::warn!(error = %err, "failed to update session activity");
            }

            let _ = outbound.send(&user_id, &response).await;
        });
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
        agent_builder
            .clone()
            .build(registry, kernel_clone, config.max_turns())
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
    agent
        .prompt_with_turns_retry(prompt.to_string(), max_turns, DEFAULT_PROVIDER_RETRIES)
        .await
        .map_err(|err| anyhow::anyhow!(err))
}

async fn run_whatsapp_loop(
    store_path: String,
    media_root: PathBuf,
    max_media_size_bytes: u64,
    allowed_senders: Option<Vec<String>>,
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
            tracing::error!(error = %err, "WhatsApp store init failed");
            return;
        }
    };

    let (client_tx, mut client_rx) = mpsc::unbounded_channel();
    let allowed_senders = Arc::new(allowed_senders);

    let mut bot = match Bot::builder()
        .with_backend(backend)
        .with_transport_factory(TokioWebSocketTransportFactory::new())
        .with_http_client(UreqHttpClient::new())
        .on_event(move |event, client| {
            let inbound_tx = inbound_tx.clone();
            let qr_cache = qr_cache.clone();
            let client_tx = client_tx.clone();
            let media_root = media_root.clone();
            let allowed_senders = Arc::clone(&allowed_senders);
            async move {
                let _ = client_tx.send(StdArc::clone(&client));
                match event {
                    Event::PairingQrCode { code, .. } => {
                        let _ = qr_cache.send(Some(code));
                    }
                    Event::Message(message, info) => {
                        let from = info.source.sender.to_string();
                        if let Some(allowed) = allowed_senders.as_ref() {
                            if !is_allowed_sender(&from, allowed) {
                                tracing::debug!(user = %from, "WhatsApp ignored message (not in allowlist)");
                                return;
                            }
                        }
                        let text = message.text_content().unwrap_or_default().to_string();
                        let base = message.get_base_message();
                        let attachments = match extract_media_attachments(
                            &client,
                            base,
                            &media_root,
                            max_media_size_bytes,
                        )
                        .await
                        {
                            Ok(items) => items,
                            Err(err) => {
                                tracing::warn!(error = %err, "WhatsApp media download failed");
                                Vec::new()
                            }
                        };
                        if text.trim().is_empty() && attachments.is_empty() {
                            return;
                        }
                        let _ = inbound_tx.send(InboundMessage {
                            channel_id: "whatsapp".to_string(),
                            user_id: from,
                            text,
                            message_id: Some(info.id.to_string()),
                            attachments,
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
            tracing::error!(error = %err, "WhatsApp bot build failed");
            return;
        }
    };

    let mut run_task = tokio::spawn(async move {
        match bot.run().await {
            Ok(handle) => {
                if let Err(err) = handle.await {
                    tracing::error!(error = %err, "WhatsApp bot task error");
                }
            }
            Err(err) => {
                tracing::error!(error = %err, "WhatsApp bot error");
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

fn format_attachments_prompt(attachments: &[MediaAttachment]) -> String {
    if attachments.is_empty() {
        return String::new();
    }
    let mut lines = Vec::new();
    lines.push(
        "User sent attachments (use multimodal_looker for images, documents, audio, or video if needed):"
            .to_string(),
    );
    for (idx, attachment) in attachments.iter().enumerate() {
        let label = format!("{}. {}", idx + 1, attachment_label(attachment));
        lines.push(label);
    }
    lines.join("\n")
}

fn attachment_label(attachment: &MediaAttachment) -> String {
    let kind = match attachment.media_type {
        MediaType::Image => "image",
        MediaType::Document => "document",
        MediaType::Audio => "audio",
        MediaType::Video => "video",
        MediaType::Sticker => "sticker",
    };
    let mut parts = Vec::new();
    parts.push(format!("type={kind}"));
    parts.push(format!("path={}", attachment.local_path.display()));
    if let Some(name) = &attachment.file_name {
        parts.push(format!("name={name}"));
    }
    if let Some(mime) = &attachment.mime_type {
        parts.push(format!("mime={mime}"));
    }
    if let Some(size) = attachment.size_bytes {
        parts.push(format!("bytes={size}"));
    }
    if let Some(caption) = &attachment.caption {
        parts.push(format!("caption={caption}"));
    }
    if let Some(thumbnail_path) = &attachment.thumbnail_path {
        parts.push(format!("thumbnail_path={}", thumbnail_path.display()));
    }
    if let Some(thumbnail_mime) = &attachment.thumbnail_mime_type {
        parts.push(format!("thumbnail_mime={thumbnail_mime}"));
    }
    if let Some(thumbnail_size) = attachment.thumbnail_size_bytes {
        parts.push(format!("thumbnail_bytes={thumbnail_size}"));
    }
    parts.join(" ")
}

fn with_media_permissions(kernel: Arc<Kernel>, attachments: &[MediaAttachment]) -> Arc<Kernel> {
    if attachments.is_empty() {
        return kernel;
    }
    let mut profile = kernel.prompt_profile().clone();
    for attachment in attachments {
        let path = PathPattern(attachment.local_path.to_string_lossy().to_string());
        profile
            .pre_authorized
            .insert(Permission::FileRead { path: path.clone() });
        profile.max_allowed.insert(Permission::FileRead { path });
        if let Some(thumbnail_path) = &attachment.thumbnail_path {
            let path = PathPattern(thumbnail_path.to_string_lossy().to_string());
            profile
                .pre_authorized
                .insert(Permission::FileRead { path: path.clone() });
            profile.max_allowed.insert(Permission::FileRead { path });
        }
    }
    Arc::new(kernel.as_ref().clone().with_prompt_profile(profile))
}

pub fn whatsapp_media_root(config: &Config, channel: &WhatsappConfig) -> PathBuf {
    if let Some(path) = &channel.store_path {
        let base = PathBuf::from(path);
        if let Some(parent) = base.parent() {
            return parent.join("whatsapp-media");
        }
    }
    config.data_dir().join("whatsapp-media")
}

fn ensure_media_dir(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)
        .with_context(|| format!("failed to create media dir at {}", root.display()))
}

async fn cleanup_expired_media(root: &Path, retention_hours: u64) {
    let retention = Duration::from_secs(retention_hours.saturating_mul(60 * 60));
    if retention.as_secs() == 0 {
        return;
    }
    let cutoff = SystemTime::now().checked_sub(retention);
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        if metadata.is_dir() {
            if let Ok(dir_entries) = std::fs::read_dir(&path) {
                let mut remove_dir = true;
                for inner in dir_entries.flatten() {
                    if should_delete(&inner.path(), cutoff) {
                        let _ = std::fs::remove_file(inner.path());
                    } else {
                        remove_dir = false;
                    }
                }
                if remove_dir {
                    let _ = std::fs::remove_dir(&path);
                }
            }
        } else if should_delete(&path, cutoff) {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn should_delete(path: &Path, cutoff: Option<SystemTime>) -> bool {
    let cutoff = match cutoff {
        Some(cutoff) => cutoff,
        None => return false,
    };
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if let Ok(modified) = metadata.modified() {
        return modified < cutoff;
    }
    false
}

async fn extract_media_attachments(
    client: &Arc<whatsapp_rust::Client>,
    message: &waproto::whatsapp::Message,
    media_root: &Path,
    max_media_size_bytes: u64,
) -> Result<Vec<MediaAttachment>> {
    let mut attachments = Vec::new();
    let base = message.get_base_message();
    if let Some(msg) = base.image_message.as_deref()
        && let Some(attachment) = download_media(
            client,
            msg,
            media_root,
            max_media_size_bytes,
            MediaMeta {
                media_type: MediaType::Image,
                mime_type: msg.mimetype.clone(),
                file_name: None,
                caption: msg.caption.clone(),
                file_length: msg.file_length,
                thumbnail_bytes: msg.jpeg_thumbnail.clone(),
            },
        )
        .await?
    {
        attachments.push(attachment);
    }
    if let Some(msg) = base.document_message.as_deref()
        && let Some(attachment) = download_media(
            client,
            msg,
            media_root,
            max_media_size_bytes,
            MediaMeta {
                media_type: MediaType::Document,
                mime_type: msg.mimetype.clone(),
                file_name: msg.file_name.clone(),
                caption: msg.caption.clone(),
                file_length: msg.file_length,
                thumbnail_bytes: msg.jpeg_thumbnail.clone(),
            },
        )
        .await?
    {
        attachments.push(attachment);
    }
    if let Some(msg) = base.audio_message.as_deref()
        && let Some(attachment) = download_media(
            client,
            msg,
            media_root,
            max_media_size_bytes,
            MediaMeta {
                media_type: MediaType::Audio,
                mime_type: msg.mimetype.clone(),
                file_name: None,
                caption: None,
                file_length: msg.file_length,
                thumbnail_bytes: None,
            },
        )
        .await?
    {
        attachments.push(attachment);
    }
    if let Some(msg) = base.video_message.as_deref()
        && let Some(attachment) = download_media(
            client,
            msg,
            media_root,
            max_media_size_bytes,
            MediaMeta {
                media_type: MediaType::Video,
                mime_type: msg.mimetype.clone(),
                file_name: None,
                caption: msg.caption.clone(),
                file_length: msg.file_length,
                thumbnail_bytes: msg.jpeg_thumbnail.clone(),
            },
        )
        .await?
    {
        attachments.push(attachment);
    }
    if let Some(msg) = base.sticker_message.as_deref()
        && let Some(attachment) = download_media(
            client,
            msg,
            media_root,
            max_media_size_bytes,
            MediaMeta {
                media_type: MediaType::Sticker,
                mime_type: msg.mimetype.clone(),
                file_name: None,
                caption: None,
                file_length: msg.file_length,
                thumbnail_bytes: None,
            },
        )
        .await?
    {
        attachments.push(attachment);
    }
    Ok(attachments)
}

#[derive(Debug, Clone)]
struct MediaMeta {
    media_type: MediaType,
    mime_type: Option<String>,
    file_name: Option<String>,
    caption: Option<String>,
    file_length: Option<u64>,
    thumbnail_bytes: Option<Vec<u8>>,
}

async fn download_media<T: whatsapp_rust::download::Downloadable>(
    client: &Arc<whatsapp_rust::Client>,
    media: &T,
    media_root: &Path,
    max_media_size_bytes: u64,
    meta: MediaMeta,
) -> Result<Option<MediaAttachment>> {
    if let Some(len) = meta.file_length
        && len > max_media_size_bytes
    {
        return Ok(None);
    }
    let extension = file_extension_from_mime(meta.mime_type.as_deref());
    let dir = media_root.join(Uuid::new_v4().to_string());
    std::fs::create_dir_all(&dir)?;
    let sanitized_name = meta.file_name.as_deref().and_then(sanitize_filename);
    let filename = sanitized_name.clone().unwrap_or_else(|| {
        let base = Uuid::new_v4().to_string();
        match extension {
            Some(ext) => format!("{base}.{ext}"),
            None => base,
        }
    });
    let path = dir.join(filename);
    let file = std::fs::File::create(&path)?;
    client.download_to_file(media, file).await?;
    let size_bytes = std::fs::metadata(&path).ok().map(|meta| meta.len());
    if let Some(size) = size_bytes
        && size > max_media_size_bytes
    {
        let _ = std::fs::remove_file(&path);
        return Ok(None);
    }
    let local_path = path.canonicalize().unwrap_or(path);
    let (thumbnail_path, thumbnail_size_bytes, thumbnail_mime_type) =
        match meta.thumbnail_bytes {
            Some(bytes) if !bytes.is_empty() => {
                let thumb_path = dir.join("thumbnail.jpg");
                if std::fs::write(&thumb_path, &bytes).is_ok() {
                    let thumb_size = Some(bytes.len() as u64);
                    let thumb_path = thumb_path.canonicalize().unwrap_or(thumb_path);
                    (Some(thumb_path), thumb_size, Some("image/jpeg".to_string()))
                } else {
                    (None, None, None)
                }
            }
            _ => (None, None, None),
        };
    Ok(Some(MediaAttachment {
        media_type: meta.media_type,
        mime_type: meta.mime_type,
        file_name: sanitized_name,
        local_path,
        caption: meta.caption,
        size_bytes,
        thumbnail_path,
        thumbnail_mime_type,
        thumbnail_size_bytes,
    }))
}

fn file_extension_from_mime(mime: Option<&str>) -> Option<String> {
    let mime = mime?.to_ascii_lowercase();
    let ext = match mime.as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "text/plain" => "txt",
        "audio/ogg" => "ogg",
        "audio/mpeg" => "mp3",
        "audio/wav" => "wav",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        _ => return None,
    };
    Some(ext.to_string())
}

fn sanitize_filename(value: &str) -> Option<String> {
    let name = Path::new(value).file_name()?.to_string_lossy().to_string();
    if name.trim().is_empty() {
        None
    } else {
        Some(name)
    }
}

fn render_qr_code(payload: &str) -> String {
    let code = match QrCode::new(payload) {
        Ok(code) => code,
        Err(_) => return payload.to_string(),
    };
    code.render::<unicode::Dense1x2>().quiet_zone(true).build()
}
