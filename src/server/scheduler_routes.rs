use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};
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
                .into_response()
        }
    };
    let identity = api_key_identity(&headers).unwrap_or_else(|| "anonymous".to_string());
    let prefix = identity.chars().take(8).collect::<String>();
    let mut user_id = payload
        .user_id
        .unwrap_or_else(|| format!("api:{prefix}"));
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

pub async fn list_schedules(
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
    let scheduler = match state.scheduler.as_ref() {
        Some(scheduler) => scheduler,
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(ErrorResponse {
                    error: "scheduler disabled".to_string(),
                }),
            )
                .into_response()
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
