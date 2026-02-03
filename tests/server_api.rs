use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use picobot::channels::config::profile_from_config;
use picobot::channels::permissions::ChannelPermissionProfile;
use picobot::config::{AuthConfig, Config, ServerConfig};
use picobot::kernel::agent::Kernel;
use picobot::kernel::permissions::{CapabilitySet, PermissionTier};
use picobot::models::router::ModelRegistry;
use picobot::server::app::build_router;
use picobot::server::state::AppState;
use picobot::session::manager::SessionManager;
use picobot::tools::builtin::register_builtin_tools;

fn build_test_state() -> AppState {
    let config = Config {
        agent: None,
        models: vec![picobot::config::ModelConfig {
            id: "default".to_string(),
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            api_key_env: None,
            base_url: None,
        }],
        routing: Some(picobot::config::RoutingConfig {
            default: Some("default".to_string()),
        }),
        permissions: None,
        logging: None,
        server: None,
        channels: None,
        session: None,
    };

    let registry = ModelRegistry::from_config(&config).expect("registry");
    let tools = register_builtin_tools(config.permissions.as_ref()).expect("tools");
    let kernel = Arc::new(Kernel::new(tools, std::path::PathBuf::from("."))
        .with_capabilities(CapabilitySet::empty()));
    let api_profile = default_profile();
    let server_config = ServerConfig {
        bind: None,
        expose_externally: None,
        auth: Some(AuthConfig {
            api_keys: vec!["test-key".to_string()],
        }),
        cors: None,
    };

    AppState {
        kernel,
        models: Arc::new(registry),
        sessions: Arc::new(SessionManager::new()),
        api_profile,
        server_config: Some(server_config),
        snapshot_path: None,
        max_tool_rounds: 2,
        channel_type: picobot::channels::adapter::ChannelType::Api,
    }
}

fn default_profile() -> ChannelPermissionProfile {
    profile_from_config(None, PermissionTier::UserGrantable).expect("profile")
}

#[tokio::test]
async fn chat_endpoint_requires_api_key() {
    let state = build_test_state();
    let app = build_router(state);

    let payload = serde_json::json!({
        "message": "hello",
        "user_id": "tester"
    });
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/chat")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn chat_stream_endpoint_requires_api_key() {
    let state = build_test_state();
    let app = build_router(state);

    let payload = serde_json::json!({
        "message": "hello",
        "user_id": "tester"
    });
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/chat/stream")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();

    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
