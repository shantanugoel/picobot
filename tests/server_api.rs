use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use picobot::channels::config::profile_from_config;
use picobot::channels::permissions::ChannelPermissionProfile;
use picobot::config::{AuthConfig, Config, ServerConfig};
use picobot::delivery::tracking::DeliveryTracker;
use picobot::kernel::agent::Kernel;
use picobot::kernel::permissions::{CapabilitySet, PermissionTier};
use picobot::models::router::ModelRegistry;
use picobot::server::app::build_router;
use picobot::server::state::AppState;
use picobot::session::persistent_manager::PersistentSessionManager;
use picobot::tools::builtin::register_builtin_tools;
use uuid::Uuid;

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
        data: None,
        scheduler: None,
        notifications: None,
        heartbeats: None,
    };

    let registry = ModelRegistry::from_config(&config).expect("registry");
    let temp_dir = std::env::temp_dir().join(format!("picobot-tools-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let tools_dir = temp_dir.to_string_lossy().to_string();
    let tools = register_builtin_tools(config.permissions.as_ref(), Some(tools_dir.as_str()))
        .expect("tools");
    let kernel = Arc::new(
        Kernel::new(tools, std::path::PathBuf::from(".")).with_capabilities(CapabilitySet::empty()),
    );
    let api_profile = default_profile();
    let websocket_profile = default_profile();
    let deliveries = DeliveryTracker::new();
    let server_config = ServerConfig {
        bind: None,
        expose_externally: None,
        auth: Some(AuthConfig {
            api_keys: vec!["test-key".to_string()],
        }),
        cors: None,
        rate_limit: None,
    };

    let sessions = Arc::new(PersistentSessionManager::new(temp_store()));

    AppState {
        kernel,
        models: Arc::new(registry),
        sessions,
        deliveries,
        api_profile,
        websocket_profile,
        server_config: Some(server_config),
        rate_limiter: None,
        snapshot_path: None,
        max_tool_rounds: 2,
        channel_type: picobot::channels::adapter::ChannelType::Api,
        whatsapp_qr: None,
        whatsapp_qr_cache: None,
        scheduler: None,
    }
}

fn temp_store() -> picobot::session::db::SqliteStore {
    let dir = std::env::temp_dir().join(format!("picobot-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("conversations.db");
    let store = picobot::session::db::SqliteStore::new(path.to_string_lossy().to_string());
    store.touch().unwrap();
    store
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

#[tokio::test]
async fn session_access_requires_ownership() {
    let state = build_test_state();
    let session = state
        .sessions
        .create_session(
            "session-ownership".to_string(),
            picobot::channels::adapter::ChannelType::Api,
            "api".to_string(),
            "api:other".to_string(),
            &default_profile(),
        )
        .unwrap();
    let _ = state.sessions.update_session(&session);
    let app = build_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/api/v1/sessions/session-ownership")
        .header("x-api-key", "test-key")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn list_sessions_scoped_to_api_key() {
    let state = build_test_state();
    let own_session = state
        .sessions
        .create_session(
            "session-own".to_string(),
            picobot::channels::adapter::ChannelType::Api,
            "api".to_string(),
            "api:test-key".to_string(),
            &default_profile(),
        )
        .unwrap();
    let other_session = state
        .sessions
        .create_session(
            "session-other".to_string(),
            picobot::channels::adapter::ChannelType::Api,
            "api".to_string(),
            "api:other".to_string(),
            &default_profile(),
        )
        .unwrap();
    let _ = state.sessions.update_session(&own_session);
    let _ = state.sessions.update_session(&other_session);
    let app = build_router(state);

    let request = Request::builder()
        .method("GET")
        .uri("/api/v1/sessions")
        .header("x-api-key", "test-key")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let items = json.as_array().unwrap();
    assert_eq!(items.len(), 1);
}
