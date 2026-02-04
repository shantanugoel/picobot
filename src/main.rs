use std::cell::RefCell;
use std::fs;
use std::rc::Rc;
use std::sync::{Arc, Mutex, mpsc};

use picobot::channels::config::profile_from_config;
use picobot::channels::permissions::ChannelPermissionProfile;
use picobot::channels::whatsapp::{
    WhatsAppBackend, WhatsAppInboundAdapter, WhatsAppOutboundSender, WhatsappRustBackend,
};
use picobot::cli::format_permissions;
use picobot::cli::tui::{ModelChoice, PermissionChoice, Tui, TuiEvent};
use picobot::config::Config;
use picobot::delivery::queue::{DeliveryQueue, DeliveryQueueConfig};
use picobot::delivery::tracking::DeliveryTracker;
use picobot::kernel::agent::Kernel;
use picobot::kernel::agent_loop::{
    ConversationState, PermissionDecision, run_agent_loop_streamed_with_permissions_limit,
};
use picobot::kernel::permissions::CapabilitySet;
use picobot::kernel::privacy::{PrivacyController, PurgeScope};
use picobot::models::router::ModelRegistry;
use picobot::notifications::queue::{NotificationQueue, NotificationQueueConfig};
use picobot::notifications::service::NotificationService;
use picobot::notifications::whatsapp::WhatsAppNotificationChannel;
use picobot::scheduler::executor::JobExecutor;
use picobot::scheduler::service::SchedulerService;
use picobot::scheduler::store::ScheduleStore;
use picobot::heartbeats::register_heartbeats;
use picobot::server::app::{bind_address, build_router, is_localhost_only};
use picobot::server::rate_limit::RateLimiter;
use picobot::server::snapshot::spawn_snapshot_task;
use picobot::server::state::{AppState, maybe_start_retention};
use picobot::session::persistent_manager::PersistentSessionManager;
use picobot::session::snapshot::SnapshotStore;
use picobot::tools::builtin::register_builtin_tools;
use tokio::sync::{broadcast, watch};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = load_config().unwrap_or_default();
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 && args[1] == "serve" {
        return run_server(config).await;
    }

    let registry = match ModelRegistry::from_config(&config) {
        Ok(registry) => registry,
        Err(err) => {
            eprintln!("Model registry error: {err}");
            return Ok(());
        }
    };

    let data_dir = config.data.as_ref().and_then(|data| data.dir.as_deref());
    let tool_registry = register_builtin_tools(config.permissions.as_ref(), data_dir)?;
    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let capabilities = config
        .permissions
        .as_ref()
        .map(CapabilitySet::from_config)
        .unwrap_or_else(CapabilitySet::empty);
    let memory_config = config
        .session
        .as_ref()
        .and_then(|session| session.memory.clone())
        .unwrap_or_default();
    let memory_store = picobot::session::db::SqliteStore::new(
        std::path::PathBuf::from(data_dir.unwrap_or("data"))
            .join("conversations.db")
            .to_string_lossy()
            .to_string(),
    );
    let mut kernel = if memory_config.enable_user_memories.unwrap_or(true) {
        Kernel::new(tool_registry, working_dir)
            .with_capabilities(capabilities)
            .with_memory_retriever(picobot::kernel::memory::MemoryRetriever::new(
                memory_config,
                memory_store,
            ))
    } else {
        Kernel::new(tool_registry, working_dir).with_capabilities(capabilities)
    };
    kernel.set_scheduler(None);
    let kernel = Arc::new(kernel);

    let mut state = ConversationState::new();
    if let Some(agent) = &config.agent
        && let Some(system_prompt) = &agent.system_prompt
    {
        state.push(picobot::models::types::Message::system(
            system_prompt.clone(),
        ));
    }

    let registry = Arc::new(registry);
    run_tui(kernel, registry, state, config.permissions.as_ref()).await
}

async fn run_server(config: Config) -> anyhow::Result<()> {
    let registry = match ModelRegistry::from_config(&config) {
        Ok(registry) => registry,
        Err(err) => {
            eprintln!("Model registry error: {err}");
            return Ok(());
        }
    };

    let data_dir = config.data.as_ref().and_then(|data| data.dir.as_deref());
    let tool_registry = register_builtin_tools(config.permissions.as_ref(), data_dir)?;
    let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let capabilities = config
        .permissions
        .as_ref()
        .map(CapabilitySet::from_config)
        .unwrap_or_else(CapabilitySet::empty);
    let memory_config = config
        .session
        .as_ref()
        .and_then(|session| session.memory.clone())
        .unwrap_or_default();
    let memory_store = picobot::session::db::SqliteStore::new(
        std::path::PathBuf::from(data_dir.unwrap_or("data"))
            .join("conversations.db")
            .to_string_lossy()
            .to_string(),
    );
    let mut kernel = if memory_config.enable_user_memories.unwrap_or(true) {
        Kernel::new(tool_registry, working_dir)
            .with_capabilities(capabilities)
            .with_memory_retriever(picobot::kernel::memory::MemoryRetriever::new(
                memory_config,
                memory_store,
            ))
    } else {
        Kernel::new(tool_registry, working_dir).with_capabilities(capabilities)
    };
    kernel.set_scheduler(None);
    let kernel = Arc::new(kernel);

    let api_profile = api_profile_from_config(&config)?;
    let websocket_profile = websocket_profile_from_config(&config)?;
    let whatsapp_profile = whatsapp_profile_from_config(&config)?;
    let session_store_path = std::path::PathBuf::from(data_dir.unwrap_or("data"))
        .join("conversations.db")
        .to_string_lossy()
        .to_string();
    let session_store = picobot::session::db::SqliteStore::new(session_store_path);
    let _ = session_store.touch();
    let sessions = Arc::new(PersistentSessionManager::new(session_store));
    let deliveries = DeliveryTracker::new();
    let (whatsapp_backend, _whatsapp_qr, _whatsapp_qr_cache, whatsapp_allowed_senders) =
        setup_whatsapp_backend(&config);
    let snapshot_path = config
        .session
        .as_ref()
        .and_then(|session| session.snapshot_path.clone());
    let mut snapshot_store = None;
    if let Some(path) = snapshot_path.clone() {
        let store_path = std::path::PathBuf::from(data_dir.unwrap_or("data"))
            .join("conversations.db")
            .to_string_lossy()
            .to_string();
        let store = picobot::session::db::SqliteStore::new(store_path);
        let _ = store.touch();
        let _ = picobot::session::migration::migrate_snapshot_if_present(&store, &path);
        snapshot_store = Some(SnapshotStore::new(path));
    }

    let rate_limiter = config
        .server
        .as_ref()
        .and_then(|server| server.rate_limit.as_ref())
        .and_then(RateLimiter::from_config);
    let models = Arc::new(registry);
    let scheduler_config = config.scheduler.clone().unwrap_or_default();
    let scheduler = if scheduler_config.enabled() {
        let schedule_store = ScheduleStore::new(picobot::session::db::SqliteStore::new(
            std::path::PathBuf::from(data_dir.unwrap_or("data"))
                .join("conversations.db")
                .to_string_lossy()
                .to_string(),
        ));
        let _ = schedule_store.store().touch();
        let executor = JobExecutor::new(
            Arc::clone(&kernel),
            Arc::clone(&models),
            schedule_store.clone(),
            scheduler_config.clone(),
        );
        if config
            .notifications
            .as_ref()
            .map(|value| value.enabled())
            .unwrap_or(false)
            && let Some(backend) = whatsapp_backend.as_ref()
        {
            let queue_config = config
                .notifications
                .as_ref()
                .map(|value| NotificationQueueConfig {
                    max_attempts: value.max_attempts(),
                    base_backoff: std::time::Duration::from_millis(value.base_backoff_ms()),
                    max_backoff: std::time::Duration::from_millis(value.max_backoff_ms()),
                })
                .unwrap_or_default();
            let queue = NotificationQueue::new(queue_config);
            let channel = Arc::new(WhatsAppNotificationChannel::new(Arc::new(
                WhatsAppOutboundSender::new(Arc::clone(backend)),
            )));
            let notifications = NotificationService::new(queue, channel);
            let worker = notifications.clone();
            tokio::spawn(async move {
                worker.worker_loop().await;
            });
            executor.set_notifications(Some(notifications)).await;
        }
        let service = Arc::new(SchedulerService::new(
            schedule_store,
            executor,
            scheduler_config.clone(),
        ));
        if let Some(heartbeats) = config.heartbeats.as_ref() {
            let summary = register_heartbeats(&service, heartbeats);
            if summary.created > 0 || summary.skipped_existing > 0 {
                println!(
                    "Heartbeats: {} created, {} existing, {} skipped",
                    summary.created, summary.skipped_existing, summary.skipped_disabled
                );
            }
        }
        let service_clone = Arc::clone(&service);
        tokio::spawn(async move {
            service_clone.run_loop().await;
        });
        Some(service)
    } else {
        None
    };

    if let Some(service) = scheduler.as_ref()
        && let Ok(mut slot) = kernel.context().scheduler.write()
    {
        *slot = Some(Arc::clone(service));
    }

    let state = AppState {
        kernel,
        models,
        sessions,
        deliveries: deliveries.clone(),
        api_profile,
        websocket_profile,
        server_config: config.server.clone(),
        rate_limiter,
        snapshot_path,
        max_tool_rounds: 8,
        channel_type: picobot::channels::adapter::ChannelType::Api,
        whatsapp_qr: _whatsapp_qr,
        whatsapp_qr_cache: _whatsapp_qr_cache,
        scheduler,
    };

    maybe_start_retention(&config, &state.models);
    if let Some(store) = snapshot_store {
        let interval_secs = config
            .session
            .as_ref()
            .and_then(|session| session.snapshot_interval_secs)
            .unwrap_or(300);
        spawn_snapshot_task(Arc::clone(&state.sessions), store, interval_secs);
    }

    if let Some(backend) = whatsapp_backend {
        let delivery_queue = DeliveryQueue::new(deliveries.clone(), DeliveryQueueConfig::default());
        let delivery_worker = delivery_queue.clone();
        let outbound = Arc::new(WhatsAppOutboundSender::new(Arc::clone(&backend)));
        tokio::spawn(async move {
            delivery_worker.worker_loop(outbound).await;
        });
        let whatsapp_profile = whatsapp_profile.clone();
        let inbound = Arc::new(WhatsAppInboundAdapter::new(
            Arc::clone(&backend),
            whatsapp_allowed_senders,
        ));
        let sessions = Arc::clone(&state.sessions);
        let kernel = Arc::clone(&state.kernel);
        let models = Arc::clone(&state.models);
        let profile = whatsapp_profile;
        let max_tool_rounds = state.max_tool_rounds;
        let runtime = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            runtime.block_on(async move {
                if let Err(err) = backend.start().await {
                    eprintln!("WhatsApp backend failed: {err}");
                    return;
                }
                picobot::server::runtime::run_adapter_loop(
                    inbound,
                    delivery_queue,
                    sessions,
                    kernel,
                    models,
                    profile,
                    max_tool_rounds,
                )
                .await;
            });
        });
    }

    let app = build_router(state.clone());
    let addr = bind_address(&state);
    if !is_localhost_only(&state) {
        eprintln!("Warning: server is configured to expose externally.");
    }
    println!("Starting server on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .await
        .map_err(|err| anyhow::anyhow!(err))
}

async fn run_tui(
    kernel: Arc<Kernel>,
    registry: Arc<ModelRegistry>,
    mut state: ConversationState,
    permissions: Option<&picobot::config::PermissionsConfig>,
) -> anyhow::Result<()> {
    let tui = Rc::new(RefCell::new(
        Tui::new().map_err(|err| anyhow::anyhow!(err))?,
    ));
    let mut current_model_id = registry.default_id().to_string();
    {
        if let Ok(mut ui) = tui.try_borrow_mut() {
            if let Some(model) = registry.get(&current_model_id) {
                let info = model.info();
                ui.set_current_model(format!("{} ({})", info.provider, info.model));
            }
            ui.refresh().ok();
        }
    }
    let env_config = load_config().unwrap_or_default();
    let ws_url = std::env::var("PICOBOT_WS_URL").ok();
    let ws_api_key = std::env::var("PICOBOT_WS_API_KEY").ok();
    let ws = ws_url.map(|url| picobot::cli::ws_client::spawn_ws_client(url, ws_api_key));
    let mut ws_session_id: Option<String> = None;

    loop {
        if let Some(ws) = ws.as_ref() {
            loop {
                match ws.inbound.try_recv() {
                    Ok(picobot::cli::ws_client::WsUiMessage::WhatsappQr(code)) => {
                        if let Ok(mut ui) = tui.try_borrow_mut() {
                            ui.set_pending_qr(code);
                            ui.refresh().ok();
                        }
                    }
                    Ok(_) => {}
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        }

        let event = {
            let mut ui = tui.borrow_mut();
            ui.next_event().map_err(|err| anyhow::anyhow!(err))?
        };
        match event {
            TuiEvent::Quit => break,
            TuiEvent::None => {}
            TuiEvent::Input(line) => {
                if line == "/clear" {
                    let mut ui = tui.borrow_mut();
                    ui.clear_output();
                    ui.refresh().ok();
                    continue;
                }
                if line == "/help" {
                    let mut ui = tui.borrow_mut();
                    ui.push_output(
                        "Commands: /help /quit /exit /clear /permissions /models /purge_session /purge_user /purge_older <days>",
                    );
                    ui.refresh().ok();
                    continue;
                }
                if line == "/purge_session"
                    || line == "/purge_user"
                    || line.starts_with("/purge_older")
                {
                    if let Ok(ui) = tui.try_borrow()
                        && ui.is_busy()
                    {
                        continue;
                    }
                    let data_dir_value = std::env::var("PICOBOT_DATA_DIR")
                        .ok()
                        .or_else(|| std::env::var("DATA_DIR").ok())
                        .or_else(|| env_config.data.as_ref().and_then(|data| data.dir.clone()));
                    let data_dir = data_dir_value.as_deref();
                    let mut parts = line.split_whitespace();
                    let command = parts.next().unwrap_or_default();
                    let days = parts.next().and_then(|value| value.parse::<u32>().ok());
                    let scope = match command {
                        "/purge_session" => Some(PurgeScope::Session),
                        "/purge_user" => Some(PurgeScope::User),
                        "/purge_older" => Some(PurgeScope::OlderThanDays),
                        _ => None,
                    };
                    let Some(scope) = scope else {
                        continue;
                    };
                    if let Ok(mut ui) = tui.try_borrow_mut() {
                        ui.set_pending_confirmation("Confirm purge? (y/n)");
                        ui.refresh().ok();
                    }
                    loop {
                        let event = {
                            let mut ui = tui.borrow_mut();
                            ui.next_event_with_timeout(std::time::Duration::from_millis(80))
                                .map_err(|err| anyhow::anyhow!(err))?
                        };
                        if let TuiEvent::Permission(choice) = event {
                            if let Ok(mut ui) = tui.try_borrow_mut() {
                                ui.clear_pending_permission();
                                match choice {
                                    PermissionChoice::Once | PermissionChoice::Session => {
                                        if matches!(scope, PurgeScope::OlderThanDays)
                                            && days.is_none()
                                        {
                                            ui.push_output(
                                                "Missing days for /purge_older".to_string(),
                                            );
                                            break;
                                        }
                                        let store_path =
                                            std::path::PathBuf::from(data_dir.unwrap_or("data"))
                                                .join("conversations.db")
                                                .to_string_lossy()
                                                .to_string();
                                        let store =
                                            picobot::session::db::SqliteStore::new(store_path);
                                        let sessions =
                                            Arc::new(PersistentSessionManager::new(store));
                                        let controller = PrivacyController::new(sessions);
                                        let ctx = kernel.context();
                                        let result = controller.purge(ctx, scope, days);
                                        if let Err(err) = result {
                                            ui.push_output(format!("Purge failed: {err}"));
                                        } else {
                                            ui.push_output("Purge completed".to_string());
                                        }
                                    }
                                    PermissionChoice::Deny => {
                                        ui.push_output("Purge cancelled".to_string());
                                    }
                                }
                                ui.refresh().ok();
                            }
                            break;
                        }
                    }
                    continue;
                }
                if line == "/permissions" {
                    let mut ui = tui.borrow_mut();
                    ui.push_output(format_permissions(permissions));
                    ui.refresh().ok();
                    continue;
                }
                if line == "/models" {
                    let models = registry
                        .model_infos()
                        .into_iter()
                        .map(|info| ModelChoice {
                            id: info.id.clone(),
                            label: format!("{}: {} ({})", info.id, info.provider, info.model),
                        })
                        .collect();
                    let mut ui = tui.borrow_mut();
                    ui.set_pending_model_picker(models);
                    ui.refresh().ok();
                    continue;
                }
                if line.trim().is_empty() {
                    continue;
                }

                {
                    let mut ui = tui.borrow_mut();
                    ui.push_user(format!("> {line}"));
                    ui.start_assistant_message();
                    ui.set_busy(true);
                    ui.refresh().ok();
                }

                if let Some(ws) = ws.as_ref() {
                    let _ = ws
                        .outbound
                        .send(picobot::channels::websocket::WsClientMessage::Chat {
                            session_id: ws_session_id.clone(),
                            user_id: Some("tui".to_string()),
                            message: line.clone(),
                            model: Some(current_model_id.clone()),
                        });
                    let mut buffer = String::new();
                    loop {
                        match ws.inbound.try_recv() {
                            Ok(message) => match message {
                                picobot::cli::ws_client::WsUiMessage::Session(id) => {
                                    ws_session_id = Some(id);
                                }
                                picobot::cli::ws_client::WsUiMessage::WhatsappQr(code) => {
                                    if let Ok(mut ui) = tui.try_borrow_mut() {
                                        ui.set_pending_qr(code);
                                        ui.refresh().ok();
                                    }
                                }
                                picobot::cli::ws_client::WsUiMessage::Token(token) => {
                                    buffer.push_str(&token);
                                    if let Ok(mut ui) = tui.try_borrow_mut() {
                                        ui.append_output(&token);
                                    }
                                }
                                picobot::cli::ws_client::WsUiMessage::PermissionRequired {
                                    tool,
                                    permissions,
                                    request_id,
                                } => {
                                    if let Ok(mut ui) = tui.try_borrow_mut() {
                                        ui.set_pending_permission(tool, permissions);
                                        ui.set_busy(false);
                                        ui.refresh().ok();
                                    }
                                    loop {
                                        let event = {
                                            let mut ui = tui.borrow_mut();
                                            ui.next_event_with_timeout(
                                                std::time::Duration::from_millis(80),
                                            )
                                            .map_err(|err| anyhow::anyhow!(err))?
                                        };
                                        if let TuiEvent::Permission(choice) = event {
                                            if let Ok(mut ui) = tui.try_borrow_mut() {
                                                ui.clear_pending_permission();
                                                ui.set_busy(true);
                                                ui.refresh().ok();
                                            }
                                            let decision = match choice {
                                                PermissionChoice::Once => picobot::channels::websocket::PermissionDecisionChoice::Once,
                                                PermissionChoice::Session => picobot::channels::websocket::PermissionDecisionChoice::Session,
                                                PermissionChoice::Deny => picobot::channels::websocket::PermissionDecisionChoice::Deny,
                                            };
                                            let _ = ws.outbound.send(picobot::channels::websocket::WsClientMessage::PermissionDecision {
                                                request_id,
                                                decision,
                                            });
                                            break;
                                        }
                                    }
                                }
                                picobot::cli::ws_client::WsUiMessage::Done(_) => {
                                    if let Ok(mut ui) = tui.try_borrow_mut() {
                                        ui.set_busy(false);
                                        if !buffer.ends_with('\n') {
                                            ui.push_assistant("".to_string());
                                        }
                                        ui.refresh().ok();
                                    }
                                    break;
                                }
                                picobot::cli::ws_client::WsUiMessage::Error(err) => {
                                    if let Ok(mut ui) = tui.try_borrow_mut() {
                                        ui.set_busy(false);
                                        ui.push_output(format!("Error: {err}"));
                                        ui.refresh().ok();
                                    }
                                    break;
                                }
                            },
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                if let Ok(mut ui) = tui.try_borrow_mut() {
                                    ui.set_busy(false);
                                    ui.push_output("WS connection closed".to_string());
                                    ui.refresh().ok();
                                }
                                break;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                        }
                        let event = {
                            let mut ui = tui.borrow_mut();
                            ui.next_event_with_timeout(std::time::Duration::from_millis(80))
                                .map_err(|err| anyhow::anyhow!(err))?
                        };
                        match event {
                            TuiEvent::ModelPick(model_id) => {
                                current_model_id = model_id.clone();
                                if let Ok(mut ui) = tui.try_borrow_mut() {
                                    if let Some(model) = registry.get(&current_model_id) {
                                        let info = model.info();
                                        ui.set_current_model(format!(
                                            "{} ({})",
                                            info.provider, info.model
                                        ));
                                        ui.push_output(format!("Switched to model '{}'", info.id));
                                    } else {
                                        ui.push_output("Unknown model id".to_string());
                                    }
                                    ui.clear_pending_model_picker();
                                    ui.refresh().ok();
                                }
                            }
                            TuiEvent::Quit => break,
                            _ => {}
                        }
                    }
                } else {
                    let model = registry
                        .get_arc(&current_model_id)
                        .unwrap_or_else(|| registry.default_model_arc());
                    let mut buffer = String::new();

                    let (tx, rx) = mpsc::channel::<UiMessage>();
                    let decision_slot: Arc<Mutex<Option<PermissionDecision>>> =
                        Arc::new(Mutex::new(None));
                    let decision_worker = Arc::clone(&decision_slot);

                    let kernel_worker = Arc::clone(&kernel);
                    let model_worker = model.clone();
                    let mut state_value = std::mem::take(&mut state);
                    let line_value = line.clone();

                    std::thread::spawn(move || {
                        let mut on_token = |token: &str| {
                            let _ = tx.send(UiMessage::Token(token.to_string()));
                        };
                        let mut on_debug = |line: &str| {
                            let _ = tx.send(UiMessage::Debug(line.to_string()));
                        };
                        let mut on_permission =
                            |tool: &str, required: &[picobot::kernel::permissions::Permission]| {
                                let permissions =
                                    required.iter().map(|perm| format!("{perm:?}")).collect();
                                let _ =
                                    tx.send(UiMessage::Permission(tool.to_string(), permissions));
                                loop {
                                    if let Ok(mut slot) = decision_worker.lock()
                                        && let Some(decision) = slot.take()
                                    {
                                        return decision;
                                    }
                                    std::thread::sleep(std::time::Duration::from_millis(30));
                                }
                            };

                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build();
                        let result = match rt {
                            Ok(rt) => rt.block_on(run_agent_loop_streamed_with_permissions_limit(
                                kernel_worker.as_ref(),
                                model_worker.as_ref(),
                                &mut state_value,
                                line_value,
                                &mut on_token,
                                &mut on_permission,
                                &mut on_debug,
                                8,
                            )),
                            Err(err) => Err(picobot::tools::traits::ToolError::ExecutionFailed(
                                err.to_string(),
                            )),
                        };
                        let _ = tx.send(UiMessage::Done(result, state_value));
                    });

                    loop {
                        if let Ok(message) = rx.try_recv() {
                            match message {
                                UiMessage::Token(token) => {
                                    buffer.push_str(&token);
                                    if let Ok(mut ui) = tui.try_borrow_mut() {
                                        ui.append_output(&token);
                                    }
                                }
                                UiMessage::Debug(line) => {
                                    if let Ok(mut ui) = tui.try_borrow_mut() {
                                        ui.push_debug(line);
                                    }
                                }
                                UiMessage::Permission(tool, permissions) => {
                                    if let Ok(mut ui) = tui.try_borrow_mut() {
                                        ui.set_pending_permission(tool, permissions);
                                        ui.set_busy(false);
                                        ui.refresh().ok();
                                    }
                                }
                                UiMessage::Done(result, updated_state) => {
                                    state = updated_state;
                                    if let Ok(mut ui) = tui.try_borrow_mut() {
                                        ui.set_busy(false);
                                        if let Err(err) = result {
                                            ui.push_output(format!("Error: {err}"));
                                        } else if !buffer.ends_with('\n') {
                                            ui.push_assistant("".to_string());
                                        }
                                        ui.refresh().ok();
                                    }
                                    break;
                                }
                            }
                        }

                        let event = {
                            let mut ui = tui.borrow_mut();
                            ui.next_event_with_timeout(std::time::Duration::from_millis(80))
                                .map_err(|err| anyhow::anyhow!(err))?
                        };
                        match event {
                            TuiEvent::Permission(choice) => {
                                if let Ok(mut ui) = tui.try_borrow_mut() {
                                    ui.clear_pending_permission();
                                    ui.set_busy(true);
                                    ui.refresh().ok();
                                }
                                let decision = match choice {
                                    PermissionChoice::Once => PermissionDecision::Once,
                                    PermissionChoice::Session => PermissionDecision::Session,
                                    PermissionChoice::Deny => PermissionDecision::Deny,
                                };
                                if let Ok(mut slot) = decision_slot.lock() {
                                    *slot = Some(decision);
                                }
                            }
                            TuiEvent::ModelPick(model_id) => {
                                current_model_id = model_id.clone();
                                if let Ok(mut ui) = tui.try_borrow_mut() {
                                    if let Some(model) = registry.get(&current_model_id) {
                                        let info = model.info();
                                        ui.set_current_model(format!(
                                            "{} ({})",
                                            info.provider, info.model
                                        ));
                                        ui.push_output(format!("Switched to model '{}'", info.id));
                                    } else {
                                        ui.push_output("Unknown model id".to_string());
                                    }
                                    ui.clear_pending_model_picker();
                                    ui.refresh().ok();
                                }
                            }
                            TuiEvent::Quit => {
                                break;
                            }
                            _ => {}
                        }
                    }
                }
            }
            TuiEvent::ModelPick(model_id) => {
                current_model_id = model_id.clone();
                if let Ok(mut ui) = tui.try_borrow_mut() {
                    if let Some(model) = registry.get(&current_model_id) {
                        let info = model.info();
                        ui.set_current_model(format!("{} ({})", info.provider, info.model));
                        ui.push_output(format!("Switched to model '{}'", info.id));
                    } else {
                        ui.push_output("Unknown model id".to_string());
                    }
                    ui.clear_pending_model_picker();
                    ui.refresh().ok();
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn load_config() -> Option<Config> {
    let raw = fs::read_to_string("config.toml").ok()?;
    toml::from_str(&raw).ok()
}

fn api_profile_from_config(config: &Config) -> Result<ChannelPermissionProfile, anyhow::Error> {
    let api_config = config
        .channels
        .as_ref()
        .and_then(|channels| channels.api.as_ref());
    profile_from_config(
        api_config,
        picobot::kernel::permissions::PermissionTier::UserGrantable,
    )
    .map_err(|err| anyhow::anyhow!(err))
}

fn websocket_profile_from_config(
    config: &Config,
) -> Result<ChannelPermissionProfile, anyhow::Error> {
    let ws_config = config
        .channels
        .as_ref()
        .and_then(|channels| channels.websocket.as_ref());
    profile_from_config(
        ws_config,
        picobot::kernel::permissions::PermissionTier::UserGrantable,
    )
    .map_err(|err| anyhow::anyhow!(err))
}

fn whatsapp_profile_from_config(
    config: &Config,
) -> Result<ChannelPermissionProfile, anyhow::Error> {
    let wa_config = config
        .channels
        .as_ref()
        .and_then(|channels| channels.whatsapp.as_ref());
    profile_from_config(
        wa_config,
        picobot::kernel::permissions::PermissionTier::AdminOnly,
    )
    .map_err(|err| anyhow::anyhow!(err))
}

type WhatsappSetup = (
    Option<Arc<dyn WhatsAppBackend>>,
    Option<broadcast::Sender<String>>,
    Option<watch::Receiver<Option<String>>>,
    Option<Vec<String>>,
);

fn setup_whatsapp_backend(config: &Config) -> WhatsappSetup {
    let Some(channel) = config
        .channels
        .as_ref()
        .and_then(|channels| channels.whatsapp.as_ref())
    else {
        return (None, None, None, None);
    };
    if channel.enabled == Some(false) {
        return (None, None, None, None);
    }
    let allowed_senders = if channel.allowed_senders.is_empty() {
        None
    } else {
        Some(channel.allowed_senders.clone())
    };
    let store_path = channel
        .store_path
        .clone()
        .unwrap_or_else(|| "./data/whatsapp.db".to_string());
    let (qr_tx, _) = broadcast::channel(4);
    let (qr_cache_tx, qr_cache_rx) = watch::channel(None);
    let backend = Arc::new(WhatsappRustBackend::new(
        store_path,
        Some(qr_tx.clone()),
        Some(qr_cache_tx),
    ));
    (
        Some(backend),
        Some(qr_tx),
        Some(qr_cache_rx),
        allowed_senders,
    )
}

enum UiMessage {
    Token(String),
    Debug(String),
    Permission(String, Vec<String>),
    Done(
        Result<String, picobot::tools::traits::ToolError>,
        ConversationState,
    ),
}
