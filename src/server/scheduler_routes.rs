use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};
use crate::scheduler::job::ExecutionStatus;
use crate::server::middleware::{api_key_identity, check_api_key};
use crate::server::state::AppState;

use super::routes::ErrorResponse;

#[derive(Debug, Deserialize)]
pub struct CreateScheduleRequest {
    pub name: String,
    pub schedule_type: ScheduleType,
    pub schedule_expr: String,
    pub task_prompt: String,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
    pub channel_id: Option<String>,
    pub enabled: Option<bool>,
    pub max_executions: Option<u32>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct ScheduleResponse {
    pub id: String,
    pub name: String,
    pub schedule_type: ScheduleType,
    pub schedule_expr: String,
    pub task_prompt: String,
    pub session_id: Option<String>,
    pub user_id: String,
    pub channel_id: Option<String>,
    pub enabled: bool,
    pub max_executions: Option<u32>,
    pub execution_count: u32,
    pub next_run_at: chrono::DateTime<chrono::Utc>,
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub consecutive_failures: u32,
    pub last_error: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateScheduleRequest {
    pub name: Option<String>,
    pub schedule_type: Option<ScheduleType>,
    pub schedule_expr: Option<String>,
    pub task_prompt: Option<String>,
    pub session_id: Option<String>,
    pub channel_id: Option<String>,
    pub enabled: Option<bool>,
    pub max_executions: Option<u32>,
    pub metadata: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
pub struct ExecutionQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ExecutionResponse {
    pub id: String,
    pub job_id: String,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub status: ExecutionStatus,
    pub result_summary: Option<String>,
    pub error: Option<String>,
    pub execution_time_ms: Option<i64>,
}

pub async fn create_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateScheduleRequest>,
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
    let scheduler = match state.scheduler.as_ref() {
        Some(scheduler) => scheduler,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "scheduler disabled".to_string(),
                }),
            )
                .into_response();
        }
    };
    let identity = api_key_identity(&headers).unwrap_or_else(|| "anonymous".to_string());
    let prefix = identity.chars().take(8).collect::<String>();
    let mut user_id = payload.user_id.unwrap_or_else(|| format!("api:{prefix}"));
    if !user_id.starts_with("api:") {
        user_id = format!("api:{user_id}");
    }
    let capabilities = if let Some(session_id) = payload.session_id.as_ref() {
        match state.sessions.get_session(session_id) {
            Ok(Some(session)) => {
                if session.user_id != user_id {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(ErrorResponse {
                            error: "session not owned by user".to_string(),
                        }),
                    )
                        .into_response();
                }
                session.permissions
            }
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(ErrorResponse {
                        error: "session not found".to_string(),
                    }),
                )
                    .into_response();
            }
            Err(err) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: err.to_string(),
                    }),
                )
                    .into_response();
            }
        }
    } else {
        state.api_profile.grants()
    };
    let request = CreateJobRequest {
        name: payload.name,
        schedule_type: payload.schedule_type,
        schedule_expr: payload.schedule_expr,
        task_prompt: payload.task_prompt,
        session_id: payload.session_id,
        user_id: user_id.clone(),
        channel_id: payload.channel_id,
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
    match scheduler.create_job(request) {
        Ok(job) => (
            StatusCode::OK,
            Json(ScheduleResponse {
                id: job.id,
                name: job.name,
                schedule_type: job.schedule_type,
                schedule_expr: job.schedule_expr,
                task_prompt: job.task_prompt,
                session_id: job.session_id,
                user_id: job.user_id,
                channel_id: job.channel_id,
                enabled: job.enabled,
                max_executions: job.max_executions,
                execution_count: job.execution_count,
                next_run_at: job.next_run_at,
                last_run_at: job.last_run_at,
                consecutive_failures: job.consecutive_failures,
                last_error: job.last_error,
                created_at: job.created_at,
            }),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn list_schedules(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = check_api_key(
        &headers,
        state
            .server_config
            .as_ref()
            .and_then(|cfg| cfg.auth.as_ref()),
    ) {
        return *response;
    }
    let scheduler = match state.scheduler.as_ref() {
        Some(scheduler) => scheduler,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "scheduler disabled".to_string(),
                }),
            )
                .into_response();
        }
    };
    let identity = api_key_identity(&headers).unwrap_or_else(|| "anonymous".to_string());
    let prefix = identity.chars().take(8).collect::<String>();
    let user_id = format!("api:{prefix}");
    match scheduler.list_jobs_by_user(&user_id) {
        Ok(jobs) => {
            let response = jobs
                .into_iter()
                .map(|job| ScheduleResponse {
                    id: job.id,
                    name: job.name,
                    schedule_type: job.schedule_type,
                    schedule_expr: job.schedule_expr,
                    task_prompt: job.task_prompt,
                    session_id: job.session_id,
                    user_id: job.user_id,
                    channel_id: job.channel_id,
                    enabled: job.enabled,
                    max_executions: job.max_executions,
                    execution_count: job.execution_count,
                    next_run_at: job.next_run_at,
                    last_run_at: job.last_run_at,
                    consecutive_failures: job.consecutive_failures,
                    last_error: job.last_error,
                    created_at: job.created_at,
                })
                .collect::<Vec<_>>();
            (StatusCode::OK, Json(response)).into_response()
        }
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn get_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
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
    let scheduler = match state.scheduler.as_ref() {
        Some(scheduler) => scheduler,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "scheduler disabled".to_string(),
                }),
            )
                .into_response();
        }
    };
    let identity = api_key_identity(&headers).unwrap_or_else(|| "anonymous".to_string());
    let prefix = identity.chars().take(8).collect::<String>();
    let user_id = format!("api:{prefix}");
    let job = match scheduler.get_job(&id) {
        Ok(job) => job,
        Err(err) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    if job.user_id != user_id {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "schedule not owned by api key".to_string(),
            }),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        Json(ScheduleResponse {
            id: job.id,
            name: job.name,
            schedule_type: job.schedule_type,
            schedule_expr: job.schedule_expr,
            task_prompt: job.task_prompt,
            session_id: job.session_id,
            user_id: job.user_id,
            channel_id: job.channel_id,
            enabled: job.enabled,
            max_executions: job.max_executions,
            execution_count: job.execution_count,
            next_run_at: job.next_run_at,
            last_run_at: job.last_run_at,
            consecutive_failures: job.consecutive_failures,
            last_error: job.last_error,
            created_at: job.created_at,
        }),
    )
        .into_response()
}

pub async fn update_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(payload): Json<UpdateScheduleRequest>,
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
    let scheduler = match state.scheduler.as_ref() {
        Some(scheduler) => scheduler,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "scheduler disabled".to_string(),
                }),
            )
                .into_response();
        }
    };
    let identity = api_key_identity(&headers).unwrap_or_else(|| "anonymous".to_string());
    let prefix = identity.chars().take(8).collect::<String>();
    let user_id = format!("api:{prefix}");
    let mut job = match scheduler.get_job(&id) {
        Ok(job) => job,
        Err(err) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    if job.user_id != user_id {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "schedule not owned by api key".to_string(),
            }),
        )
            .into_response();
    }
    if payload.schedule_type.is_some() && payload.schedule_expr.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "schedule_expr required when updating schedule_type".to_string(),
            }),
        )
            .into_response();
    }
    let mut update_next_run = false;
    if let Some(value) = payload.name {
        job.name = value;
    }
    if let Some(value) = payload.schedule_type {
        job.schedule_type = value;
        update_next_run = true;
    }
    if let Some(value) = payload.schedule_expr {
        job.schedule_expr = value;
        update_next_run = true;
    }
    if let Some(value) = payload.task_prompt {
        job.task_prompt = value;
    }
    if let Some(value) = payload.session_id {
        job.session_id = if value.is_empty() { None } else { Some(value) };
    }
    if let Some(value) = payload.channel_id {
        job.channel_id = if value.is_empty() { None } else { Some(value) };
    }
    if let Some(value) = payload.enabled {
        job.enabled = value;
    }
    if let Some(value) = payload.max_executions {
        job.max_executions = Some(value);
    }
    if let Some(value) = payload.metadata {
        job.metadata = Some(value);
    }
    if update_next_run {
        match crate::scheduler::service::compute_next_run_for(job.schedule_type, &job.schedule_expr)
        {
            Ok(next) => job.next_run_at = next,
            Err(err) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: err.to_string(),
                    }),
                )
                    .into_response();
            }
        }
    }
    job.updated_at = chrono::Utc::now();
    if let Err(err) = scheduler.update_job(&job) {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response();
    }
    (
        StatusCode::OK,
        Json(ScheduleResponse {
            id: job.id,
            name: job.name,
            schedule_type: job.schedule_type,
            schedule_expr: job.schedule_expr,
            task_prompt: job.task_prompt,
            session_id: job.session_id,
            user_id: job.user_id,
            channel_id: job.channel_id,
            enabled: job.enabled,
            max_executions: job.max_executions,
            execution_count: job.execution_count,
            next_run_at: job.next_run_at,
            last_run_at: job.last_run_at,
            consecutive_failures: job.consecutive_failures,
            last_error: job.last_error,
            created_at: job.created_at,
        }),
    )
        .into_response()
}

pub async fn delete_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
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
    let scheduler = match state.scheduler.as_ref() {
        Some(scheduler) => scheduler,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "scheduler disabled".to_string(),
                }),
            )
                .into_response();
        }
    };
    let identity = api_key_identity(&headers).unwrap_or_else(|| "anonymous".to_string());
    let prefix = identity.chars().take(8).collect::<String>();
    let user_id = format!("api:{prefix}");
    let job = match scheduler.get_job(&id) {
        Ok(job) => job,
        Err(err) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    if job.user_id != user_id {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "schedule not owned by api key".to_string(),
            }),
        )
            .into_response();
    }
    let _ = scheduler.delete_job_with_cancel(&id);
    StatusCode::NO_CONTENT.into_response()
}

pub async fn list_schedule_executions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<ExecutionQuery>,
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
    let scheduler = match state.scheduler.as_ref() {
        Some(scheduler) => scheduler,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "scheduler disabled".to_string(),
                }),
            )
                .into_response();
        }
    };
    let identity = api_key_identity(&headers).unwrap_or_else(|| "anonymous".to_string());
    let prefix = identity.chars().take(8).collect::<String>();
    let user_id = format!("api:{prefix}");
    let job = match scheduler.get_job(&id) {
        Ok(job) => job,
        Err(err) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    if job.user_id != user_id {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "schedule not owned by api key".to_string(),
            }),
        )
            .into_response();
    }
    let limit = query.limit.unwrap_or(50).min(200);
    let offset = query.offset.unwrap_or(0);
    match scheduler.list_executions_for_job(&id, limit, offset) {
        Ok(executions) => (
            StatusCode::OK,
            Json(
                executions
                    .into_iter()
                    .map(|execution| ExecutionResponse {
                        id: execution.id,
                        job_id: execution.job_id,
                        started_at: execution.started_at,
                        completed_at: execution.completed_at,
                        status: execution.status,
                        result_summary: execution.result_summary,
                        error: execution.error,
                        execution_time_ms: execution.execution_time_ms,
                    })
                    .collect::<Vec<_>>(),
            ),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}

pub async fn cancel_schedule(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
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
    let scheduler = match state.scheduler.as_ref() {
        Some(scheduler) => scheduler,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "scheduler disabled".to_string(),
                }),
            )
                .into_response();
        }
    };
    let identity = api_key_identity(&headers).unwrap_or_else(|| "anonymous".to_string());
    let prefix = identity.chars().take(8).collect::<String>();
    let user_id = format!("api:{prefix}");
    let job = match scheduler.get_job(&id) {
        Ok(job) => job,
        Err(err) => {
            return (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: err.to_string(),
                }),
            )
                .into_response();
        }
    };
    if job.user_id != user_id {
        return (
            StatusCode::FORBIDDEN,
            Json(ErrorResponse {
                error: "schedule not owned by api key".to_string(),
            }),
        )
            .into_response();
    }
    match scheduler.cancel_job(&id) {
        Ok(cancelled) => (
            StatusCode::OK,
            Json(serde_json::json!({"status": "cancelled", "running": cancelled})),
        )
            .into_response(),
        Err(err) => (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: err.to_string(),
            }),
        )
            .into_response(),
    }
}
