use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use picobot::channels::api;
use picobot::config::{ApiAuthConfig, ApiConfig, Config};
use picobot::kernel::core::Kernel;
use picobot::kernel::permissions::{CapabilitySet, Permission};
use picobot::providers::factory::ProviderAgentBuilder;
use picobot::scheduler::executor::JobExecutor;
use picobot::scheduler::service::SchedulerService;
use picobot::scheduler::store::ScheduleStore;
use picobot::session::db::SqliteStore;
use picobot::tools::registry::ToolRegistry;

fn build_test_config() -> Config {
    let mut config = Config::default();
    config.api = Some(ApiConfig {
        auth: Some(ApiAuthConfig {
            api_keys: vec![
                "test-key".to_string(),
                "user1:api:user1".to_string(),
                "user2:api:user2".to_string(),
            ],
        }),
        rate_limit: None,
        max_body_bytes: Some(1_048_576),
    });
    config.provider = Some("openai".to_string());
    config.model = Some("gpt-4o-mini".to_string());
    config.data_dir = Some(temp_path("picobot-data").to_string_lossy().to_string());
    config
}

fn build_kernel() -> Kernel {
    let registry = ToolRegistry::new();
    Kernel::new(std::sync::Arc::new(registry))
}

fn build_kernel_with_scheduler(config: &Config) -> Kernel {
    let registry = ToolRegistry::new();
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let capabilities = CapabilitySet::from_config_with_base(&config.permissions(), &base_dir);
    let kernel = Kernel::new(std::sync::Arc::new(registry)).with_capabilities(capabilities);
    let store_path = temp_path("picobot-schedule").join("picobot.db");
    std::fs::create_dir_all(store_path.parent().unwrap()).unwrap();
    let store = SqliteStore::new(store_path.to_string_lossy().to_string());
    store.touch().unwrap();
    let schedule_store = ScheduleStore::new(store.clone());
    let agent_builder = ProviderAgentBuilder::new(config).unwrap();
    let scheduler_config = config.scheduler().clone();
    let executor = JobExecutor::new(
        std::sync::Arc::new(kernel.clone()),
        schedule_store.clone(),
        scheduler_config.clone(),
        agent_builder,
        None,
        config.clone(),
    );
    let scheduler = SchedulerService::new(schedule_store, executor, scheduler_config);
    kernel.with_scheduler(Some(std::sync::Arc::new(scheduler)))
}

fn temp_path(prefix: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()))
}

#[tokio::test]
async fn prompt_requires_api_key() {
    let config = build_test_config();
    let kernel = build_kernel();
    let agent_builder = ProviderAgentBuilder::new(&config).unwrap();
    let (_addr, app) = api::router(config, kernel, agent_builder).unwrap();

    let payload = serde_json::json!({
        "prompt": "hello"
    });
    let request = Request::builder()
        .method("POST")
        .uri("/v1/prompt")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn schedule_cancel_without_scheduler_returns_unavailable() {
    let config = build_test_config();
    let kernel = build_kernel();
    let agent_builder = ProviderAgentBuilder::new(&config).unwrap();
    let (_addr, app) = api::router(config, kernel, agent_builder).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri("/v1/schedules/abc123/cancel")
        .header("x-api-key", "test-key")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn schedule_list_requires_schedule_permission() {
    let mut config = build_test_config();
    let mut scheduler_config = picobot::config::SchedulerConfig::default();
    scheduler_config.enabled = Some(true);
    config.scheduler = Some(scheduler_config);
    let kernel = build_kernel_with_scheduler(&config);
    let agent_builder = ProviderAgentBuilder::new(&config).unwrap();
    let (_addr, app) = api::router(config, kernel, agent_builder).unwrap();
    let request = Request::builder()
        .method("GET")
        .uri("/v1/schedules")
        .header("x-api-key", "test-key")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn schedule_cancel_rejects_non_owner() {
    let mut config = build_test_config();
    let mut scheduler_config = picobot::config::SchedulerConfig::default();
    scheduler_config.enabled = Some(true);
    config.scheduler = Some(scheduler_config);
    config.permissions = Some(picobot::config::PermissionsConfig {
        schedule: Some(picobot::config::SchedulePermissions {
            allowed_actions: vec![
                "cancel".to_string(),
                "list".to_string(),
                "create".to_string(),
            ],
        }),
        ..Default::default()
    });
    let base_kernel = build_kernel_with_scheduler(&config);
    let user1 = "api:user1".to_string();
    let kernel = base_kernel.clone_with_context(Some(user1.clone()), Some("api:user1".to_string()));
    let scheduler = kernel.context().scheduler.clone().unwrap();
    let mut capabilities = CapabilitySet::empty();
    capabilities.insert(Permission::Schedule {
        action: "create".to_string(),
    });
    let request = picobot::scheduler::job::CreateJobRequest {
        name: "job".to_string(),
        schedule_type: picobot::scheduler::job::ScheduleType::Interval,
        schedule_expr: "60".to_string(),
        task_prompt: "ping".to_string(),
        session_id: Some("api:user1".to_string()),
        user_id: user1.clone(),
        channel_id: Some("api".to_string()),
        capabilities,
        creator: picobot::scheduler::job::Principal {
            principal_type: picobot::scheduler::job::PrincipalType::User,
            id: user1.clone(),
        },
        enabled: true,
        max_executions: None,
        created_by_system: false,
        metadata: None,
    };
    let job = scheduler.create_job(request).unwrap();

    let agent_builder = ProviderAgentBuilder::new(&config).unwrap();
    let (_addr, app) = api::router(config, base_kernel, agent_builder).unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/v1/schedules/{}/cancel", job.id))
        .header("x-api-key", "user2")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn chat_requires_api_key() {
    let config = build_test_config();
    let kernel = build_kernel();
    let agent_builder = ProviderAgentBuilder::new(&config).unwrap();
    let (_addr, app) = api::router(config, kernel, agent_builder).unwrap();
    let payload = serde_json::json!({
        "message": "hello"
    });
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn chat_rejects_mismatched_session_id() {
    let config = build_test_config();
    let kernel = build_kernel();
    let agent_builder = ProviderAgentBuilder::new(&config).unwrap();
    let (_addr, app) = api::router(config, kernel, agent_builder).unwrap();
    let payload = serde_json::json!({
        "message": "hello",
        "session_id": "api:other"
    });
    let request = Request::builder()
        .method("POST")
        .uri("/v1/chat")
        .header("content-type", "application/json")
        .header("x-api-key", "test-key")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn rate_limit_returns_429() {
    let mut config = build_test_config();
    config.api = Some(ApiConfig {
        auth: config.api.as_ref().and_then(|api| api.auth.clone()),
        rate_limit: Some(picobot::config::ApiRateLimitConfig {
            requests_per_minute: Some(2),
        }),
        max_body_bytes: Some(1_048_576),
    });
    let kernel = build_kernel();
    let agent_builder = ProviderAgentBuilder::new(&config).unwrap();
    let (_addr, app) = api::router(config, kernel, agent_builder).unwrap();
    let payload = serde_json::json!({
        "prompt": "hello"
    });
    let request = |payload: &serde_json::Value| {
        Request::builder()
            .method("POST")
            .uri("/v1/prompt")
            .header("content-type", "application/json")
            .header("x-api-key", "test-key")
            .body(Body::from(payload.to_string()))
            .unwrap()
    };
    let response1 = app.clone().oneshot(request(&payload)).await.unwrap();
    assert_ne!(response1.status(), StatusCode::TOO_MANY_REQUESTS);
    let response2 = app.clone().oneshot(request(&payload)).await.unwrap();
    assert_ne!(response2.status(), StatusCode::TOO_MANY_REQUESTS);
    let response3 = app.oneshot(request(&payload)).await.unwrap();
    assert_eq!(response3.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn schedule_create_requires_permission() {
    let mut config = build_test_config();
    let mut scheduler_config = picobot::config::SchedulerConfig::default();
    scheduler_config.enabled = Some(true);
    config.scheduler = Some(scheduler_config);
    let kernel = build_kernel_with_scheduler(&config);
    let agent_builder = ProviderAgentBuilder::new(&config).unwrap();
    let (_addr, app) = api::router(config, kernel, agent_builder).unwrap();
    let payload = serde_json::json!({
        "schedule_type": "interval",
        "schedule_expr": "60",
        "task_prompt": "ping"
    });
    let request = Request::builder()
        .method("POST")
        .uri("/v1/schedules")
        .header("content-type", "application/json")
        .header("x-api-key", "test-key")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn schedule_create_succeeds_with_permission() {
    let mut config = build_test_config();
    let mut scheduler_config = picobot::config::SchedulerConfig::default();
    scheduler_config.enabled = Some(true);
    config.scheduler = Some(scheduler_config);
    config.permissions = Some(picobot::config::PermissionsConfig {
        schedule: Some(picobot::config::SchedulePermissions {
            allowed_actions: vec!["create".to_string()],
        }),
        ..Default::default()
    });
    let kernel = build_kernel_with_scheduler(&config);
    let agent_builder = ProviderAgentBuilder::new(&config).unwrap();
    let (_addr, app) = api::router(config, kernel, agent_builder).unwrap();
    let payload = serde_json::json!({
        "schedule_type": "interval",
        "schedule_expr": "60",
        "task_prompt": "ping"
    });
    let request = Request::builder()
        .method("POST")
        .uri("/v1/schedules")
        .header("content-type", "application/json")
        .header("x-api-key", "test-key")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn auth_via_bearer_token() {
    let config = build_test_config();
    let kernel = build_kernel();
    let agent_builder = ProviderAgentBuilder::new(&config).unwrap();
    let (_addr, app) = api::router(config, kernel, agent_builder).unwrap();
    let payload = serde_json::json!({
        "prompt": "hello"
    });
    let request = Request::builder()
        .method("POST")
        .uri("/v1/prompt")
        .header("content-type", "application/json")
        .header("authorization", "Bearer test-key")
        .body(Body::from(payload.to_string()))
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);
}
