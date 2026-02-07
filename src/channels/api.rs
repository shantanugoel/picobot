use std::sync::Arc;

use std::collections::HashMap;
use std::sync::Mutex;

use crate::channels::permissions::channel_profile;
use crate::providers::error::ProviderError;
use crate::providers::factory::{DEFAULT_PROVIDER_RETRIES, ProviderAgentBuilder};
use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::post,
};
use serde::{Deserialize, Serialize};
use tower_http::limit::RequestBodyLimitLayer;

use crate::config::Config;
use crate::kernel::core::Kernel;
use crate::kernel::permissions::{CapabilitySet, Permission};
use crate::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};
use crate::session::manager::SessionManager;
use crate::session::memory::MemoryRetriever;
use crate::session::types::{MessageType, StoredMessage};
use crate::tools::traits::ExecutionMode;

#[derive(Debug, Deserialize)]
struct PromptRequest {
    prompt: String,
    session_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct PromptResponse {
    response: String,
}

#[derive(Debug, Deserialize)]
struct PromptMessageRequest {
    message: String,
    session_id: Option<String>,
}

#[derive(Debug, Serialize)]
struct PromptMessageResponse {
    response: String,
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct ScheduleCreateRequest {
    name: Option<String>,
    schedule_type: String,
    schedule_expr: String,
    task_prompt: String,
    session_id: Option<String>,
    channel_id: Option<String>,
    enabled: Option<bool>,
    max_executions: Option<u32>,
    capabilities: Option<Vec<String>>,
    metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ScheduleCreateResponse {
    status: String,
    job_id: String,
    next_run_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Serialize)]
struct ScheduleItemResponse {
    id: String,
    name: String,
    schedule_type: ScheduleType,
    schedule_expr: String,
    enabled: bool,
    execution_count: u32,
    next_run_at: chrono::DateTime<chrono::Utc>,
    last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    last_error: Option<String>,
}

#[derive(Debug, Serialize)]
struct ScheduleListResponse {
    schedules: Vec<ScheduleItemResponse>,
}

#[derive(Clone)]
pub struct AppState {
    agent_builder: ProviderAgentBuilder,
    max_turns: usize,
    kernel: Arc<Kernel>,
    config: Config,
    rate_limiter: Arc<RateLimiter>,
    auth_identities: HashMap<String, String>,
    session_manager: Arc<SessionManager>,
    memory_retriever: Arc<MemoryRetriever>,
}

#[derive(Clone, Default)]
struct RateLimiter {
    inner: Arc<Mutex<HashMap<String, Vec<std::time::Instant>>>>,
}

impl RateLimiter {
    fn allow(&self, key: &str, limit: u32) -> bool {
        if limit == 0 {
            return true;
        }
        let mut guard = self.inner.lock().expect("rate limiter mutex poisoned");
        let now = std::time::Instant::now();
        let window = std::time::Duration::from_secs(60);
        let entries = guard.entry(key.to_string()).or_default();
        entries.retain(|instant| now.duration_since(*instant) <= window);
        if entries.len() >= limit as usize {
            return false;
        }
        entries.push(now);
        true
    }
}

async fn prompt_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<PromptRequest>,
) -> Result<Json<PromptResponse>, (StatusCode, String)> {
    let user_id = authenticate(&state, &headers)?;
    enforce_rate_limit(&state, &user_id)?;
    let session_id = payload
        .session_id
        .unwrap_or_else(|| default_session_id(&user_id));
    validate_session_id(&session_id, &user_id)?;
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let profile = channel_profile(&state.config.channels(), "api", &base_dir);
    let scoped_kernel = Arc::new(
        state
            .kernel
            .clone_with_context(Some(user_id.clone()), Some(session_id))
            .with_channel_id(Some("api".to_string()))
            .with_prompt_profile(profile),
    );
    let agent = build_agent_for_kernel(
        &state.config,
        &state.agent_builder,
        scoped_kernel,
        state.max_turns,
    )
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    tracing::info!(
        event = "channel_prompt",
        channel_id = "api",
        user_id = %user_id,
        prompt_len = payload.prompt.len(),
        max_turns = state.max_turns,
        "api prompt received"
    );
    let response = agent
        .prompt_with_turns_retry(
            payload.prompt.clone(),
            state.max_turns,
            DEFAULT_PROVIDER_RETRIES,
        )
        .await
        .map_err(map_provider_error)?;
    tracing::info!(
        event = "channel_prompt_complete",
        channel_id = "api",
        user_id = %user_id,
        response_len = response.len(),
        "api prompt completed"
    );
    Ok(Json(PromptResponse { response }))
}

async fn prompt_message_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<PromptMessageRequest>,
) -> Result<Json<PromptMessageResponse>, (StatusCode, String)> {
    let user_id = authenticate(&state, &headers)?;
    enforce_rate_limit(&state, &user_id)?;
    let session_id = payload
        .session_id
        .unwrap_or_else(|| default_session_id(&user_id));
    validate_session_id(&session_id, &user_id)?;
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let profile = channel_profile(&state.config.channels(), "api", &base_dir);
    let scoped_kernel = Arc::new(
        state
            .kernel
            .clone_with_context(Some(user_id.clone()), Some(session_id.clone()))
            .with_channel_id(Some("api".to_string()))
            .with_prompt_profile(profile),
    );

    let session = match state.session_manager.get_session(&session_id) {
        Ok(Some(session)) => session,
        Ok(None) => state
            .session_manager
            .create_session(
                session_id.clone(),
                "api".to_string(),
                "api".to_string(),
                user_id.clone(),
                scoped_kernel.context().capabilities.as_ref().clone(),
            )
            .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?,
        Err(err) => return Err((StatusCode::INTERNAL_SERVER_ERROR, err.to_string())),
    };

    let memory_config = state.config.memory();
    let existing_messages = state
        .session_manager
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
    let context_messages = state.memory_retriever.build_context(
        scoped_kernel.context().user_id.as_deref(),
        scoped_kernel.context().session_id.as_deref(),
        &filtered_messages,
    );
    let context_snippet = MemoryRetriever::to_prompt_snippet(&context_messages);
    let prompt_to_send = if let Some(context) = context_snippet {
        format!("Context:\n{context}\n\nUser: {}", payload.message)
    } else {
        payload.message.clone()
    };

    let mut seq_order = match state.session_manager.get_messages(&session.id, 1) {
        Ok(messages) => messages
            .last()
            .map(|message| message.seq_order + 1)
            .unwrap_or(0),
        Err(_) => 0,
    };
    let user_message = StoredMessage {
        message_type: MessageType::User,
        content: payload.message.clone(),
        tool_call_id: None,
        seq_order,
        token_estimate: None,
    };
    if state
        .session_manager
        .append_message(&session.id, &user_message)
        .is_ok()
    {
        seq_order += 1;
    }

    let agent = build_agent_for_kernel(
        &state.config,
        &state.agent_builder,
        scoped_kernel,
        state.max_turns,
    )
    .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    tracing::info!(
        event = "channel_prompt",
        channel_id = "api",
        user_id = %user_id,
        session_id = %session_id,
        prompt_len = prompt_to_send.len(),
        max_turns = state.max_turns,
        "api prompt received"
    );
    let response = agent
        .prompt_with_turns_retry(prompt_to_send, state.max_turns, DEFAULT_PROVIDER_RETRIES)
        .await
        .map_err(map_provider_error)?;
    tracing::info!(
        event = "channel_prompt_complete",
        channel_id = "api",
        user_id = %user_id,
        session_id = %session_id,
        response_len = response.len(),
        "api prompt completed"
    );

    let assistant_message = StoredMessage {
        message_type: MessageType::Assistant,
        content: response.clone(),
        tool_call_id: None,
        seq_order,
        token_estimate: None,
    };
    if let Err(err) = state
        .session_manager
        .append_message(&session.id, &assistant_message)
    {
        tracing::warn!(error = %err, "failed to store assistant message");
    }
    if let Err(err) = state.session_manager.touch(&session.id) {
        tracing::warn!(error = %err, "failed to update session activity");
    }

    Ok(Json(PromptMessageResponse {
        response,
        session_id,
    }))
}

async fn schedule_create_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<ScheduleCreateRequest>,
) -> Result<Json<ScheduleCreateResponse>, (StatusCode, String)> {
    let user_id = authenticate(&state, &headers)?;
    enforce_rate_limit(&state, &user_id)?;
    let session_id = payload
        .session_id
        .unwrap_or_else(|| default_session_id(&user_id));
    validate_session_id(&session_id, &user_id)?;
    if let Some(channel_id) = payload.channel_id.as_deref()
        && channel_id != "api"
    {
        return Err((StatusCode::BAD_REQUEST, "invalid channel_id".to_string()));
    }
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let profile = channel_profile(&state.config.channels(), "api", &base_dir);
    let scoped_kernel = state
        .kernel
        .clone_with_context(Some(user_id.clone()), Some(session_id))
        .with_channel_id(Some("api".to_string()))
        .with_prompt_profile(profile)
        .with_execution_mode(ExecutionMode::User);

    ensure_schedule_permission(
        scoped_kernel.context().capabilities.as_ref(),
        &scoped_kernel.prompt_profile().pre_authorized,
        "create",
    )?;
    let scheduler = scoped_kernel.context().scheduler.clone().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "scheduler not available".to_string(),
        )
    })?;
    let schedule_type = parse_schedule_type(&payload.schedule_type)?;
    let mut schedule_expr = payload.schedule_expr.clone();
    if matches!(schedule_type, ScheduleType::Cron) {
        schedule_expr = normalize_cron_expr(&schedule_expr)?;
    }
    let task_prompt = payload.task_prompt.clone();
    let name = payload
        .name
        .unwrap_or_else(|| default_job_name(&task_prompt));
    let requested = payload
        .capabilities
        .as_ref()
        .map(|value| parse_capabilities(value.as_slice()))
        .transpose()?;
    let capabilities = match requested {
        Some(value)
            if capabilities_subset(scoped_kernel.context().capabilities.as_ref(), &value) =>
        {
            value
        }
        _ => scoped_kernel.context().capabilities.as_ref().clone(),
    };
    let request = CreateJobRequest {
        name,
        schedule_type,
        schedule_expr,
        task_prompt,
        session_id: scoped_kernel.context().session_id.clone(),
        user_id: user_id.clone(),
        channel_id: Some("api".to_string()),
        capabilities,
        creator: Principal {
            principal_type: PrincipalType::User,
            id: user_id.clone(),
        },
        enabled: payload.enabled.unwrap_or(true),
        max_executions: payload.max_executions,
        created_by_system: false,
        metadata: payload.metadata,
    };
    let job = scheduler
        .create_job(request)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
    Ok(Json(ScheduleCreateResponse {
        status: "created".to_string(),
        job_id: job.id,
        next_run_at: job.next_run_at,
    }))
}

async fn schedule_list_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ScheduleListResponse>, (StatusCode, String)> {
    let user_id = authenticate(&state, &headers)?;
    enforce_rate_limit(&state, &user_id)?;
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let profile = channel_profile(&state.config.channels(), "api", &base_dir);
    let scoped_kernel = state
        .kernel
        .clone_with_context(Some(user_id.clone()), Some(default_session_id(&user_id)))
        .with_channel_id(Some("api".to_string()))
        .with_prompt_profile(profile);
    let scheduler = scoped_kernel.context().scheduler.clone().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "scheduler not available".to_string(),
        )
    })?;
    ensure_schedule_permission(
        scoped_kernel.context().capabilities.as_ref(),
        &scoped_kernel.prompt_profile().pre_authorized,
        "list",
    )?;
    let jobs = scheduler
        .list_jobs_by_user(&user_id)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
    let schedules = jobs
        .into_iter()
        .map(|job| ScheduleItemResponse {
            id: job.id,
            name: job.name,
            schedule_type: job.schedule_type,
            schedule_expr: job.schedule_expr,
            enabled: job.enabled,
            execution_count: job.execution_count,
            next_run_at: job.next_run_at,
            last_run_at: job.last_run_at,
            last_error: job.last_error,
        })
        .collect();
    Ok(Json(ScheduleListResponse { schedules }))
}

async fn schedule_cancel_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user_id = authenticate(&state, &headers)?;
    enforce_rate_limit(&state, &user_id)?;
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let profile = channel_profile(&state.config.channels(), "api", &base_dir);
    let scoped_kernel = state
        .kernel
        .clone_with_context(Some(user_id.clone()), Some(default_session_id(&user_id)))
        .with_channel_id(Some("api".to_string()))
        .with_prompt_profile(profile);
    let scheduler = scoped_kernel.context().scheduler.clone().ok_or_else(|| {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "scheduler not available".to_string(),
        )
    })?;
    ensure_schedule_permission(
        scoped_kernel.context().capabilities.as_ref(),
        &scoped_kernel.prompt_profile().pre_authorized,
        "cancel",
    )?;
    let job = scheduler
        .store()
        .get_job(&job_id)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "job not found".to_string()))?;
    if job.user_id != user_id {
        return Err((StatusCode::FORBIDDEN, "job not owned by user".to_string()));
    }
    scheduler
        .cancel_job_and_disable(&job_id)
        .map_err(|err| (StatusCode::BAD_REQUEST, err.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn serve(
    config: Config,
    kernel: Kernel,
    agent_builder: ProviderAgentBuilder,
) -> Result<()> {
    let (addr, router) = router(config, kernel, agent_builder)?;
    let listener = tokio::net::TcpListener::bind(addr.clone())
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    axum::serve(listener, router)
        .await
        .context("server failed")?;
    Ok(())
}

pub fn router(
    config: Config,
    kernel: Kernel,
    agent_builder: ProviderAgentBuilder,
) -> Result<(String, Router)> {
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let profile = channel_profile(&config.channels(), "api", &base_dir);
    let kernel = kernel
        .with_prompt_profile(profile)
        .with_channel_id(Some("api".to_string()));
    let api_config = config.api();
    let session_store = crate::session::db::SqliteStore::new(
        config
            .data_dir()
            .join("sessions.db")
            .to_string_lossy()
            .to_string(),
    );
    session_store.touch()?;
    let session_manager = Arc::new(SessionManager::new(session_store.clone()));
    let memory_retriever = Arc::new(MemoryRetriever::new(config.memory(), session_store));
    let state = AppState {
        agent_builder: agent_builder.clone(),
        max_turns: config.max_turns(),
        kernel: Arc::new(kernel),
        config: config.clone(),
        rate_limiter: Arc::new(RateLimiter::default()),
        auth_identities: api_auth_map(&api_config.auth().api_keys()),
        session_manager,
        memory_retriever,
    };

    let max_body = api_config.max_body_bytes();
    let app = Router::new()
        .route("/v1/prompt", post(prompt_handler))
        .route("/v1/chat", post(prompt_message_handler))
        .route("/v1/schedules", post(schedule_create_handler))
        .route("/v1/schedules", axum::routing::get(schedule_list_handler))
        .route(
            "/v1/schedules/{job_id}/cancel",
            post(schedule_cancel_handler),
        )
        .layer(RequestBodyLimitLayer::new(max_body))
        .with_state(state);

    Ok((config.bind().to_string(), app))
}

fn build_agent_for_kernel(
    config: &Config,
    agent_builder: &ProviderAgentBuilder,
    kernel: Arc<Kernel>,
    max_turns: usize,
) -> Result<crate::providers::factory::ProviderAgent> {
    let registry = kernel.tool_registry();
    if let Ok(router) = crate::providers::factory::ProviderFactory::build_agent_router(config)
        && !router.is_empty()
    {
        router.build_default(config, registry, Arc::clone(&kernel), max_turns)
    } else {
        agent_builder
            .clone()
            .build(registry, Arc::clone(&kernel), max_turns)
    }
}

fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<String, (StatusCode, String)> {
    if state.auth_identities.is_empty() {
        return Ok("api:anon".to_string());
    }
    let header = headers
        .get("x-api-key")
        .or_else(|| headers.get("authorization"))
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "missing api key".to_string()))?;
    let value = header
        .to_str()
        .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid api key".to_string()))?;
    let key = value.strip_prefix("Bearer ").unwrap_or(value);
    if let Some(identity) = state.auth_identities.get(key) {
        return Ok(identity.clone());
    }
    Err((StatusCode::UNAUTHORIZED, "invalid api key".to_string()))
}

fn enforce_rate_limit(state: &AppState, user_id: &str) -> Result<(), (StatusCode, String)> {
    let limit = state.config.api().rate_limit().requests_per_minute();
    if let Some(limit) = limit
        && !state.rate_limiter.allow(user_id, limit)
    {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded".to_string(),
        ));
    }
    Ok(())
}

fn api_auth_map(keys: &[String]) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for key in keys {
        let trimmed = key.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((token, identity)) = trimmed.split_once(':') {
            let identity = identity.trim();
            if !identity.is_empty() {
                map.insert(token.to_string(), identity.to_string());
                continue;
            }
        }
        map.insert(trimmed.to_string(), format!("api:{trimmed}"));
    }
    map
}

fn default_session_id(user_id: &str) -> String {
    format!("api:{}", session_user_segment(user_id))
}

fn validate_session_id(session_id: &str, user_id: &str) -> Result<(), (StatusCode, String)> {
    if !session_id.starts_with("api:") {
        return Err((StatusCode::BAD_REQUEST, "invalid session_id".to_string()));
    }
    let expected = session_user_segment(user_id);
    let actual = session_id.trim_start_matches("api:");
    if actual != expected {
        return Err((
            StatusCode::FORBIDDEN,
            "session_id does not match user".to_string(),
        ));
    }
    Ok(())
}

fn session_user_segment(user_id: &str) -> &str {
    user_id.strip_prefix("api:").unwrap_or(user_id)
}

fn ensure_schedule_permission(
    capabilities: &CapabilitySet,
    pre_authorized: &CapabilitySet,
    action: &str,
) -> Result<(), (StatusCode, String)> {
    let required = Permission::Schedule {
        action: action.to_string(),
    };
    let wildcard = Permission::Schedule {
        action: "*".to_string(),
    };
    if capabilities.allows(&required)
        || capabilities.allows(&wildcard)
        || pre_authorized.allows(&required)
        || pre_authorized.allows(&wildcard)
    {
        Ok(())
    } else {
        Err((
            StatusCode::FORBIDDEN,
            format!("missing schedule:{action} capability"),
        ))
    }
}

fn parse_schedule_type(value: &str) -> Result<ScheduleType, (StatusCode, String)> {
    match value {
        "interval" => Ok(ScheduleType::Interval),
        "once" => Ok(ScheduleType::Once),
        "cron" => Ok(ScheduleType::Cron),
        _ => Err((StatusCode::BAD_REQUEST, "invalid schedule_type".to_string())),
    }
}

fn parse_capabilities(
    value: &[String],
) -> Result<crate::kernel::permissions::CapabilitySet, (StatusCode, String)> {
    let mut parsed = Vec::new();
    for entry in value {
        let permission = entry
            .parse::<crate::kernel::permissions::Permission>()
            .map_err(|err| (StatusCode::BAD_REQUEST, err))?;
        parsed.push(permission);
    }
    Ok(crate::kernel::permissions::CapabilitySet::from_permissions(
        &parsed,
    ))
}

fn capabilities_subset(
    parent: &crate::kernel::permissions::CapabilitySet,
    child: &crate::kernel::permissions::CapabilitySet,
) -> bool {
    child
        .permissions()
        .all(|permission| parent.allows(permission))
}

fn default_job_name(task_prompt: &str) -> String {
    let trimmed = task_prompt.trim();
    if trimmed.is_empty() {
        return "scheduled-job".to_string();
    }
    let mut out = String::new();
    let mut last_dash = false;
    for ch in trimmed.chars() {
        if out.len() >= 40 {
            break;
        }
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "scheduled-job".to_string()
    } else {
        trimmed.to_string()
    }
}

fn normalize_cron_expr(value: &str) -> Result<String, (StatusCode, String)> {
    let trimmed = value.trim();
    let (tz, raw) = if let Some((prefix, rest)) = trimmed.split_once('|') {
        let tz = prefix.trim();
        if tz.is_empty() {
            return Err((StatusCode::BAD_REQUEST, "cron timezone missing".to_string()));
        }
        (Some(tz), rest.trim())
    } else {
        (None, trimmed)
    };
    let field_count = raw.split_whitespace().count();
    let normalized = match field_count {
        5 => format!("0 {raw}"),
        6 | 7 => raw.to_string(),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "cron expression must have 5 or 6 fields".to_string(),
            ));
        }
    };
    Ok(match tz {
        Some(tz) => format!("{tz}|{normalized}"),
        None => normalized,
    })
}

fn map_provider_error(err: ProviderError) -> (StatusCode, String) {
    let status = match err {
        ProviderError::RateLimit { .. } => StatusCode::TOO_MANY_REQUESTS,
        ProviderError::Transient { .. } => StatusCode::SERVICE_UNAVAILABLE,
        ProviderError::Permanent { .. } => StatusCode::BAD_REQUEST,
    };
    tracing::error!(error = %err, status = ?status, "prompt failed");
    (status, err.to_string())
}
