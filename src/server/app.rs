use std::net::SocketAddr;

use axum::routing::{delete, get, post};
use axum::Router;
use tower::ServiceBuilder;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::config::CorsConfig;
use crate::server::routes;
use crate::server::state::AppState;
use crate::server::ws;

pub fn build_router(state: AppState) -> Router {
    let mut app = Router::new()
        .route("/health", get(routes::health))
        .route("/status", get(routes::status))
        .route("/metrics", get(routes::metrics))
        .route("/api/v1/sessions", get(routes::list_sessions))
        .route("/api/v1/sessions/{id}", get(routes::get_session))
        .route("/api/v1/sessions/{id}", delete(routes::delete_session))
        .route("/api/v1/permissions", get(routes::permissions))
        .route("/api/v1/permissions/grant", post(routes::grant_permissions))
        .route("/api/v1/chat", post(routes::chat))
        .route("/api/v1/chat/stream", post(routes::chat_stream))
        .route("/ws", get(ws::websocket_handler))
        .with_state(state.clone());

    let cors_layer = build_cors_layer(
        state
            .server_config
            .as_ref()
            .and_then(|cfg| cfg.cors.as_ref()),
    );

    app = app.layer(
        ServiceBuilder::new()
            .layer(RequestBodyLimitLayer::new(1024 * 1024))
            .layer(TraceLayer::new_for_http()),
    );
    app = app.layer(cors_layer);

    app
}

pub fn bind_address(state: &AppState) -> SocketAddr {
    let bind = state
        .server_config
        .as_ref()
        .and_then(|cfg| cfg.bind.clone())
        .unwrap_or_else(|| "127.0.0.1:8080".to_string());
    bind.parse()
        .unwrap_or_else(|_| "127.0.0.1:8080".parse().expect("valid fallback bind"))
}

pub fn is_localhost_only(state: &AppState) -> bool {
    let expose = state
        .server_config
        .as_ref()
        .and_then(|cfg| cfg.expose_externally)
        .unwrap_or(false);
    if expose {
        return false;
    }
    state
        .server_config
        .as_ref()
        .and_then(|cfg| cfg.bind.as_ref())
        .map(|bind| bind.starts_with("127.0.0.1") || bind.starts_with("localhost"))
        .unwrap_or(true)
}

fn build_cors_layer(config: Option<&CorsConfig>) -> CorsLayer {
    let Some(config) = config else {
        return CorsLayer::new().allow_origin(Any);
    };
    if config.allowed_origins.is_empty() {
        return CorsLayer::new().allow_origin(Any);
    }
    let origins = config
        .allowed_origins
        .iter()
        .filter_map(|origin| origin.parse().ok())
        .collect::<Vec<_>>();
    CorsLayer::new().allow_origin(origins)
}
