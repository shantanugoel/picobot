use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::sync::Arc;
use tokio::task::spawn_blocking;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;

use crate::channels::adapter::ChannelType;
use crate::channels::permissions::ChannelPermissionProfile;
use crate::kernel::agent_loop::PermissionDecision;
use crate::server::metrics::{collect_metrics, render_metrics};
use crate::session::adapter::{session_from_state, state_from_session};
use crate::session::manager::{Session, SessionManager, SessionState};

use super::middleware::check_api_key;
use super::state::AppState;

fn ensure_api_key(headers: &HeaderMap, state: &AppState) -> Result<(), Box<Response>> {
    check_api_key(
        headers,
        state
            .server_config
            .as_ref()
            .and_then(|cfg| cfg.auth.as_ref()),
    )
}

async fn enforce_rate_limit(state: &AppState) -> Result<(), Response> {
    let Some(limiter) = state.rate_limiter.as_ref() else {
        return Ok(());
    };
    if limiter.check().await {
        Ok(())
    } else {
        Err((StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response())
    }
}

#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub message: String,
    pub model: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChatResponse {
    pub session_id: String,
    pub response: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct SessionDetails {
    pub id: String,
    pub channel_id: String,
    pub user_id: String,
    pub last_active: chrono::DateTime<chrono::Utc>,
    pub state: SessionState,
}

#[derive(Debug, Serialize)]
pub struct PermissionsResponse {
    pub pre_authorized: Vec<String>,
    pub max_allowed: Vec<String>,
    pub allow_user_prompts: bool,
    pub prompt_timeout_secs: u32,
}

#[derive(Debug, Serialize)]
pub struct GrantResponse {
    pub session_id: String,
    pub permissions: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GrantRequest {
    pub session_id: String,
    pub permissions: Vec<String>,
}

pub async fn health() -> &'static str {
    "ok"
}

pub async fn status(State(state): State<AppState>) -> Json<serde_json::Value> {
    let snapshot = collect_metrics(&state.sessions);
    Json(serde_json::json!({
        "sessions": snapshot.sessions_total,
        "active_sessions": snapshot.sessions_active,
        "idle_sessions": snapshot.sessions_idle,
        "awaiting_permission_sessions": snapshot.sessions_awaiting_permission,
        "terminated_sessions": snapshot.sessions_terminated,
        "channel": "api",
    }))
}

pub async fn metrics(State(state): State<AppState>) -> Response {
    let output = render_metrics(&state.sessions);
    (StatusCode::OK, output).into_response()
}

pub async fn list_sessions(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = ensure_api_key(&headers, &state) {
        return *response;
    }
    if let Err(response) = enforce_rate_limit(&state).await {
        return response;
    }

    let sessions = state.sessions.list_sessions();
    (StatusCode::OK, Json(sessions)).into_response()
}

pub async fn get_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = ensure_api_key(&headers, &state) {
        return *response;
    }
    if let Err(response) = enforce_rate_limit(&state).await {
        return response;
    }

    match state.sessions.get_session(&id) {
        Some(session) => {
            let details = SessionDetails {
                id: session.id,
                channel_id: session.channel_id,
                user_id: session.user_id,
                last_active: session.last_active,
                state: session.state,
            };
            (StatusCode::OK, Json(details)).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "session not found".to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn delete_session(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(response) = ensure_api_key(&headers, &state) {
        return *response;
    }
    if let Err(response) = enforce_rate_limit(&state).await {
        return response;
    }

    state.sessions.delete_session(&id);
    StatusCode::NO_CONTENT.into_response()
}

pub async fn permissions(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = ensure_api_key(&headers, &state) {
        return *response;
    }
    if let Err(response) = enforce_rate_limit(&state).await {
        return response;
    }

    let profile = &state.api_profile;
    let response = PermissionsResponse {
        pre_authorized: profile
            .pre_authorized
            .iter()
            .map(|perm| format!("{perm:?}"))
            .collect(),
        max_allowed: profile
            .max_allowed
            .iter()
            .map(|perm| format!("{perm:?}"))
            .collect(),
        allow_user_prompts: profile.allow_user_prompts,
        prompt_timeout_secs: profile.prompt_timeout_secs,
    };
    (StatusCode::OK, Json(response)).into_response()
}

pub async fn grant_permissions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<GrantRequest>,
) -> Response {
    if let Err(response) = ensure_api_key(&headers, &state) {
        return *response;
    }
    if let Err(response) = enforce_rate_limit(&state).await {
        return response;
    }

    let Some(mut session) = state.sessions.get_session(&payload.session_id) else {
        return (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "session not found".to_string(),
            }),
        )
            .into_response();
    };

    let mut granted = Vec::new();
    for raw in &payload.permissions {
        match raw.parse::<crate::kernel::permissions::Permission>() {
            Ok(permission) => {
                if state.api_profile.max_allowed.contains(&permission) {
                    session.permissions.insert(permission.clone());
                    granted.push(format!("{permission:?}"));
                }
            }
            Err(_) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: "invalid permission".to_string(),
                    }),
                )
                    .into_response();
            }
        }
    }
    state.sessions.update_session(session);
    (
        StatusCode::OK,
        Json(GrantResponse {
            session_id: payload.session_id.clone(),
            permissions: granted,
        }),
    )
        .into_response()
}

pub async fn chat(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ChatRequest>,
) -> Response {
    if let Err(response) = ensure_api_key(&headers, &state) {
        return *response;
    }
    if let Err(response) = enforce_rate_limit(&state).await {
        return response;
    }

    let (session_id, mut session) = load_or_create_session(
        &state.sessions,
        payload.session_id,
        payload.user_id,
        state.channel_type,
        &state.api_profile,
    );

    let mut convo_state = state_from_session(&session);
    if !payload.message.trim().is_empty() {
        convo_state.push(crate::models::types::Message::user(payload.message.clone()));
    }
    let model = match payload.model {
        Some(model_id) => state
            .models
            .get_arc(&model_id)
            .unwrap_or_else(|| state.models.default_model_arc()),
        None => state.models.default_model_arc(),
    };
    let kernel = Arc::clone(&state.kernel);
    let profile = state.api_profile.clone();
    let max_tool_rounds = state.max_tool_rounds;
    let message = payload.message.clone();

    let result = run_chat_blocking(
        kernel,
        model,
        convo_state,
        message,
        profile,
        max_tool_rounds,
        None,
    )
    .await;

    match result {
        Ok((response_text, updated_state)) => {
            session_from_state(&mut session, &updated_state);
            state.sessions.update_session(session);
            let response = ChatResponse {
                session_id,
                response: response_text,
            };
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn chat_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ChatRequest>,
) -> Response {
    if let Err(response) = ensure_api_key(&headers, &state) {
        return *response;
    }
    if let Err(response) = enforce_rate_limit(&state).await {
        return response;
    }

    let (session_id, session) = load_or_create_session(
        &state.sessions,
        payload.session_id.clone(),
        payload.user_id.clone(),
        state.channel_type,
        &state.api_profile,
    );

    let mut convo_state = state_from_session(&session);
    if !payload.message.trim().is_empty() {
        convo_state.push(crate::models::types::Message::user(payload.message.clone()));
    }

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<StreamEvent>();
    let _ = tx.send(StreamEvent::Session(session_id.clone()));

    let model = match payload.model.clone() {
        Some(model_id) => state
            .models
            .get_arc(&model_id)
            .unwrap_or_else(|| state.models.default_model_arc()),
        None => state.models.default_model_arc(),
    };
    let kernel = Arc::clone(&state.kernel);
    let sessions = Arc::clone(&state.sessions);
    let profile = state.api_profile.clone();
    let max_tool_rounds = state.max_tool_rounds;
    let message = payload.message.clone();

    spawn_blocking(move || {
        let _ = run_chat_blocking_sync(ChatExecution {
            kernel,
            model,
            convo_state,
            message,
            profile,
            max_tool_rounds,
            tx: Some(tx),
            sessions: Some(sessions),
            session: Some(session),
        });
    });

    let stream = UnboundedReceiverStream::new(rx).map(move |event| {
        let event = match event {
            StreamEvent::Session(id) => Event::default().event("session").data(id),
            StreamEvent::Token(token) => Event::default().event("token").data(token),
            StreamEvent::Done(text) => Event::default().event("done").data(
                serde_json::json!({
                    "response": text,
                    "session_id": session_id,
                })
                .to_string(),
            ),
            StreamEvent::Error(error) => Event::default()
                .event("error")
                .data(serde_json::json!({"error": error}).to_string()),
        };
        Ok::<Event, Infallible>(event)
    });

    Sse::new(stream)
        .keep_alive(KeepAlive::default())
        .into_response()
}

fn load_or_create_session(
    sessions: &SessionManager,
    session_id: Option<String>,
    user_id: Option<String>,
    channel_type: ChannelType,
    profile: &ChannelPermissionProfile,
) -> (String, Session) {
    if let Some(session_id) = session_id
        && let Some(session) = sessions.get_session(&session_id)
    {
        return (session_id, session);
    }
    let session_id = Uuid::new_v4().to_string();
    let user_id = user_id.unwrap_or_else(|| "api".to_string());
    let session = sessions.create_session(
        session_id.clone(),
        channel_type,
        "api".to_string(),
        user_id,
        profile,
    );
    (session_id, session)
}

enum StreamEvent {
    Session(String),
    Token(String),
    Done(String),
    Error(String),
}

async fn run_chat_blocking(
    kernel: Arc<crate::kernel::agent::Kernel>,
    model: Arc<dyn crate::models::traits::Model>,
    convo_state: crate::kernel::agent_loop::ConversationState,
    message: String,
    profile: ChannelPermissionProfile,
    max_tool_rounds: usize,
    tx: Option<tokio::sync::mpsc::UnboundedSender<StreamEvent>>,
) -> Result<(String, crate::kernel::agent_loop::ConversationState), String> {
    let handle = spawn_blocking(move || {
        run_chat_blocking_sync(ChatExecution {
            kernel,
            model,
            convo_state,
            message,
            profile,
            max_tool_rounds,
            tx,
            sessions: None,
            session: None,
        })
    });
    handle.await.map_err(|err| err.to_string())?
}

struct ChatExecution {
    kernel: Arc<crate::kernel::agent::Kernel>,
    model: Arc<dyn crate::models::traits::Model>,
    convo_state: crate::kernel::agent_loop::ConversationState,
    message: String,
    profile: ChannelPermissionProfile,
    max_tool_rounds: usize,
    tx: Option<tokio::sync::mpsc::UnboundedSender<StreamEvent>>,
    sessions: Option<Arc<SessionManager>>,
    session: Option<Session>,
}

fn run_chat_blocking_sync(
    exec: ChatExecution,
) -> Result<(String, crate::kernel::agent_loop::ConversationState), String> {
    let ChatExecution {
        kernel,
        model,
        mut convo_state,
        message,
        profile,
        max_tool_rounds,
        tx,
        sessions,
        mut session,
    } = exec;
    let mut response_text = String::new();
    let mut on_token = |token: &str| {
        response_text.push_str(token);
        if let Some(sender) = &tx {
            let _ = sender.send(StreamEvent::Token(token.to_string()));
        }
    };
    let mut on_permission = |_: &str, required: &[crate::kernel::permissions::Permission]| {
        if !profile.allow_user_prompts {
            return PermissionDecision::Deny;
        }
        if !profile.max_capabilities().allows_all(required) {
            return PermissionDecision::Deny;
        }
        PermissionDecision::Session
    };

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| err.to_string())?;
    let result = runtime.block_on(
        crate::kernel::agent_loop::run_agent_loop_streamed_with_permissions_limit(
            kernel.as_ref(),
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
            if let Some(session_value) = session.as_mut() {
                session_from_state(session_value, &convo_state);
                if let Some(session_store) = sessions {
                    session_store.update_session(session_value.clone());
                }
            }
            if let Some(sender) = &tx {
                let _ = sender.send(StreamEvent::Done(response_text.clone()));
            }
            Ok((response_text, convo_state))
        }
        Err(err) => {
            if let Some(sender) = &tx {
                let _ = sender.send(StreamEvent::Error(err.to_string()));
            }
            Err(err.to_string())
        }
    }
}
