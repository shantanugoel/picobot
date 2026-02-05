use std::sync::Arc;

use crate::providers::factory::ProviderAgentBuilder;
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
        .prompt_with_turns(payload.prompt, state.max_turns)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(PromptResponse { response }))
}

pub async fn serve(
    config: Config,
    kernel: Kernel,
    agent_builder: ProviderAgentBuilder,
) -> Result<()> {
    let kernel = Arc::new(kernel);
    let agent = agent_builder.build(kernel.tool_registry(), kernel.clone(), config.max_turns());
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
