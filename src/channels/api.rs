use std::sync::Arc;

use crate::channels::permissions::channel_profile;
use crate::providers::error::ProviderError;
use crate::providers::factory::{ProviderAgentBuilder, DEFAULT_PROVIDER_RETRIES};
use anyhow::{Context, Result};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::kernel::core::Kernel;

#[derive(Debug, Deserialize)]
struct PromptRequest {
    prompt: String,
}

#[derive(Debug, Serialize)]
struct PromptResponse {
    response: String,
}

#[derive(Clone)]
struct AppState {
    agent: Arc<crate::providers::factory::ProviderAgent>,
    max_turns: usize,
}

async fn prompt_handler(
    State(state): State<AppState>,
    Json(payload): Json<PromptRequest>,
) -> Result<Json<PromptResponse>, (StatusCode, String)> {
    let response = state
        .agent
        .prompt_with_turns_retry(payload.prompt, state.max_turns, DEFAULT_PROVIDER_RETRIES)
        .await
        .map_err(map_provider_error)?;
    Ok(Json(PromptResponse { response }))
}

pub async fn serve(
    config: Config,
    kernel: Kernel,
    agent_builder: ProviderAgentBuilder,
) -> Result<()> {
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let profile = channel_profile(&config.channels(), "api", &base_dir);
    let kernel = Arc::new(kernel.with_prompt_profile(profile));
    let agent = if let Ok(router) =
        crate::providers::factory::ProviderFactory::build_agent_router(&config)
        && !router.is_empty()
    {
        router.build_default(
            &config,
            kernel.tool_registry(),
            kernel.clone(),
            config.max_turns(),
        )?
    } else {
        agent_builder.build(kernel.tool_registry(), kernel.clone(), config.max_turns())?
    };
    let state = AppState {
        agent: Arc::new(agent),
        max_turns: config.max_turns(),
    };

    let app = Router::new()
        .route("/prompt", post(prompt_handler))
        .with_state(state);

    let addr = config.bind();
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("failed to bind {addr}"))?;
    axum::serve(listener, app).await.context("server failed")?;

    Ok(())
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
