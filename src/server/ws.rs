use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use futures::{SinkExt, StreamExt};
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use crate::channels::adapter::ChannelType;
use crate::channels::permissions::ChannelPermissionProfile;
use crate::channels::websocket::{PermissionDecisionChoice, WsClientMessage, WsServerMessage};
use crate::kernel::agent_loop::PermissionDecision;
use crate::server::middleware::check_api_key;
use crate::server::state::AppState;
use crate::session::adapter::{session_from_state, state_from_session};
use crate::session::manager::Session;
use crate::session::persistent_manager::PersistentSessionManager;

use super::routes::ChatRequest;

pub async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = check_api_key(
        &headers,
        state
            .server_config
            .as_ref()
            .and_then(|cfg| cfg.auth.as_ref()),
    ) {
        return *response;
    }
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsServerMessage>();
    let pending_permissions: Arc<
        Mutex<HashMap<String, std::sync::mpsc::Sender<PermissionDecision>>>,
    > = Arc::new(Mutex::new(HashMap::new()));

    if let Some(qr_rx) = state.whatsapp_qr_cache.as_ref()
        && let Some(code) = qr_rx.borrow().clone()
    {
        let _ = tx.send(WsServerMessage::WhatsappQr { code });
    }

    if let Some(qr_tx) = state.whatsapp_qr.as_ref() {
        let mut qr_rx = qr_tx.subscribe();
        let tx_clone = tx.clone();
        tokio::spawn(async move {
            loop {
                match qr_rx.recv().await {
                    Ok(code) => {
                        let _ = tx_clone.send(WsServerMessage::WhatsappQr { code });
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
    }

    let send_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let payload = match serde_json::to_string(&message) {
                Ok(payload) => payload,
                Err(_) => continue,
            };
            if sender.send(Message::Text(payload.into())).await.is_err() {
                break;
            }
        }
    });

    while let Some(Ok(message)) = receiver.next().await {
        match message {
            Message::Text(text) => {
                if let Ok(event) = serde_json::from_str::<WsClientMessage>(&text) {
                    if let Some(limiter) = state.rate_limiter.as_ref() {
                        let rate_key = match &event {
                            WsClientMessage::Chat { user_id, .. } => {
                                format!("ws:chat:{}", user_id.as_deref().unwrap_or("anonymous"))
                            }
                            WsClientMessage::PermissionDecision { .. } => {
                                "ws:permission".to_string()
                            }
                            WsClientMessage::Ping => "ws:ping".to_string(),
                        };
                        if !limiter.check_scoped(&rate_key).await {
                            let _ = tx.send(WsServerMessage::Error {
                                error: "rate limit exceeded".to_string(),
                            });
                            continue;
                        }
                    }
                    match event {
                        WsClientMessage::Ping => {
                            let _ = tx.send(WsServerMessage::Pong);
                        }
                        WsClientMessage::PermissionDecision {
                            request_id,
                            decision,
                        } => {
                            if let Ok(mut map) = pending_permissions.lock()
                                && let Some(sender) = map.remove(&request_id)
                            {
                                let mapped = match decision {
                                    PermissionDecisionChoice::Once => PermissionDecision::Once,
                                    PermissionDecisionChoice::Session => {
                                        PermissionDecision::Session
                                    }
                                    PermissionDecisionChoice::Deny => PermissionDecision::Deny,
                                };
                                let _ = sender.send(mapped);
                            }
                        }
                        WsClientMessage::Chat {
                            session_id,
                            user_id,
                            message,
                            model,
                        } => {
                            let normalized_user = user_id.map(|value| {
                                if value.starts_with("ws:") {
                                    value
                                } else {
                                    format!("ws:{value}")
                                }
                            });
                            let payload = ChatRequest {
                                session_id,
                                user_id: normalized_user,
                                message,
                                model,
                            };
                            let state_clone = state.clone();
                            let tx_clone = tx.clone();
                            let pending_clone = Arc::clone(&pending_permissions);
                            tokio::spawn(async move {
                                handle_chat(payload, state_clone, tx_clone, pending_clone).await;
                            });
                        }
                    }
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    let _ = send_task.await;
}

async fn handle_chat(
    payload: ChatRequest,
    state: AppState,
    tx: mpsc::UnboundedSender<WsServerMessage>,
    pending: Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<PermissionDecision>>>>,
) {
    let (session_id, session) = match load_or_create_session(
        &state.sessions,
        payload.session_id.clone(),
        payload.user_id.clone(),
        ChannelType::Websocket,
        &state.websocket_profile,
    ) {
        Ok(result) => result,
        Err(err) => {
            let _ = tx.send(WsServerMessage::Error {
                error: err.to_string(),
            });
            return;
        }
    };

    let mut convo_state = state_from_session(&session);
    if !payload.message.trim().is_empty() {
        convo_state.push(crate::models::types::Message::user(payload.message.clone()));
    }

    let _ = tx.send(WsServerMessage::Session {
        session_id: session_id.clone(),
    });

    let model = match payload.model.clone() {
        Some(model_id) => state
            .models
            .get_arc(&model_id)
            .unwrap_or_else(|| state.models.default_model_arc()),
        None => state.models.default_model_arc(),
    };
    let kernel = Arc::clone(&state.kernel);
    let sessions = Arc::clone(&state.sessions);
    let profile = state.websocket_profile.clone();
    let max_tool_rounds = state.max_tool_rounds;
    let message = payload.message.clone();

    tokio::task::spawn_blocking(move || {
        let _ = run_chat_blocking_sync(WsChatExecution {
            kernel,
            model,
            convo_state,
            message,
            profile,
            max_tool_rounds,
            tx,
            sessions,
            session,
            pending,
        });
    });
}

fn load_or_create_session(
    sessions: &PersistentSessionManager,
    session_id: Option<String>,
    user_id: Option<String>,
    channel_type: ChannelType,
    profile: &ChannelPermissionProfile,
) -> Result<(String, Session), String> {
    if let Some(session_id) = session_id
        && let Ok(Some(session)) = sessions.get_session(&session_id)
    {
        return Ok((session_id, session));
    }
    let session_id = Uuid::new_v4().to_string();
    let user_id = user_id.unwrap_or_else(|| "ws".to_string());
    let session = sessions
        .create_session(
            session_id.clone(),
            channel_type,
            "ws".to_string(),
            user_id,
            profile,
        )
        .map_err(|err| err.to_string())?;
    Ok((session_id, session))
}

struct WsChatExecution {
    kernel: Arc<crate::kernel::agent::Kernel>,
    model: Arc<dyn crate::models::traits::Model>,
    convo_state: crate::kernel::agent_loop::ConversationState,
    message: String,
    profile: ChannelPermissionProfile,
    max_tool_rounds: usize,
    tx: mpsc::UnboundedSender<WsServerMessage>,
    sessions: Arc<PersistentSessionManager>,
    session: Session,
    pending: Arc<Mutex<HashMap<String, std::sync::mpsc::Sender<PermissionDecision>>>>,
}

fn run_chat_blocking_sync(exec: WsChatExecution) -> Result<(), String> {
    let WsChatExecution {
        kernel,
        model,
        mut convo_state,
        message,
        profile,
        max_tool_rounds,
        tx,
        sessions,
        mut session,
        pending,
    } = exec;
    let mut response_text = String::new();
    let mut on_token = |token: &str| {
        response_text.push_str(token);
        let _ = tx.send(WsServerMessage::Token {
            token: token.to_string(),
        });
    };
    let mut on_permission = |tool: &str, required: &[crate::kernel::permissions::Permission]| {
        if !profile.allow_user_prompts {
            return PermissionDecision::Deny;
        }
        if !profile.max_capabilities().allows_all(required) {
            return PermissionDecision::Deny;
        }
        let request_id = Uuid::new_v4().to_string();
        let permissions = required.iter().map(|perm| format!("{perm:?}")).collect();
        let (sender, receiver) = std::sync::mpsc::channel();
        if let Ok(mut map) = pending.lock() {
            map.insert(request_id.clone(), sender);
        }
        let _ = tx.send(WsServerMessage::PermissionRequired {
            tool: tool.to_string(),
            permissions,
            request_id: request_id.clone(),
        });
        let timeout = Duration::from_secs(profile.prompt_timeout_secs as u64);
        let decision = match receiver.recv_timeout(timeout) {
            Ok(decision) => decision,
            Err(_) => PermissionDecision::Deny,
        };
        if let Ok(mut map) = pending.lock() {
            map.remove(&request_id);
        }
        decision
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    let scoped_kernel = kernel.clone_with_context(
        Some(session.user_id.clone()),
        Some(session.id.clone()),
    );
    let result = runtime.block_on(
        crate::kernel::agent_loop::run_agent_loop_streamed_with_permissions_limit(
            &scoped_kernel,
            model.as_ref(),
            &mut convo_state,
            message,
            &mut on_token,
            &mut on_permission,
            &mut |_| {},
            max_tool_rounds,
        ),
    );

    match result {
        Ok(text) => {
            if response_text.is_empty() {
                response_text = text;
            }
            session_from_state(&mut session, &convo_state);
            let _ = sessions.update_session(&session);
            let _ = tx.send(WsServerMessage::Done {
                response: response_text,
                session_id: session.id.clone(),
            });
            Ok(())
        }
        Err(err) => {
            let _ = tx.send(WsServerMessage::Error {
                error: err.to_string(),
            });
            Err(err.to_string())
        }
    }
}
