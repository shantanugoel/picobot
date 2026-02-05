use std::sync::Arc;

use anyhow::{Context, Result};
use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use rig::agent::Agent;
use rig::completion::Prompt;
use rig::providers::openai;
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::kernel::kernel::Kernel;
use crate::providers::factory::ProviderFactory;

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
    agent: Arc<Agent<openai::responses_api::ResponsesCompletionModel>>,
    max_turns: usize,
}

async fn prompt_handler(
    State(state): State<AppState>,
    Json(payload): Json<PromptRequest>,
) -> Result<Json<PromptResponse>, (StatusCode, String)> {
    let response = state
        .agent
        .prompt(payload.prompt)
        .max_turns(state.max_turns)
        .await
        .map_err(|err| (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;
    Ok(Json(PromptResponse { response }))
}

pub async fn serve(config: Config, kernel: Kernel) -> Result<()> {
    let kernel = Arc::new(kernel);
    let agent = ProviderFactory::build_agent(&config, kernel.tool_registry(), kernel.clone())?;
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
