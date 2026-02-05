use std::io::{self, Write};
use std::sync::Arc;

use crate::config::Config;
use crate::kernel::core::Kernel;
use crate::providers::factory::ProviderAgentBuilder;
use anyhow::{Context, Result};
use futures::StreamExt;
use rig::agent::{Agent, MultiTurnStreamItem, Text};
use rig::completion::{CompletionModel, GetTokenUsage};
use rig::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingPrompt};
use rig::wasm_compat::WasmCompatSend;

async fn stream_prompt_to_stdout<M>(
    agent: &Agent<M>,
    prompt: &str,
    max_turns: usize,
) -> Result<String>
where
    M: CompletionModel + 'static,
    <M as CompletionModel>::StreamingResponse: WasmCompatSend + GetTokenUsage,
{
    let mut response_stream = agent.stream_prompt(prompt).multi_turn(max_turns).await;
    let mut acc = String::new();
    let mut printed_any = false;
    let mut stdout = io::stdout();

    while let Some(chunk) = response_stream.next().await {
        match chunk {
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::Text(
                Text { text },
            ))) => {
                print!("{text}");
                stdout.flush().context("failed to flush stdout")?;
                acc.push_str(&text);
                printed_any = true;
            }
            Ok(MultiTurnStreamItem::StreamAssistantItem(StreamedAssistantContent::ToolCall {
                tool_call,
                ..
            })) => {
                writeln!(stdout, "\n[tool] calling {}", tool_call.function.name)
                    .context("failed to write tool call")?;
                stdout.flush().context("failed to flush stdout")?;
            }
            Ok(MultiTurnStreamItem::StreamUserItem(StreamedUserContent::ToolResult {
                tool_result,
                ..
            })) => {
                writeln!(stdout, "[tool] result {}", tool_result.id)
                    .context("failed to write tool result")?;
                stdout.flush().context("failed to flush stdout")?;
            }
            Ok(MultiTurnStreamItem::FinalResponse(final_response)) => {
                acc = final_response.response().to_string();
            }
            Err(err) => return Err(anyhow::anyhow!(err)),
            _ => {}
        }
    }

    if printed_any {
        println!();
    }
    Ok(acc)
}

pub async fn run(
    config: Config,
    kernel: Kernel,
    agent_builder: ProviderAgentBuilder,
) -> Result<()> {
    let user_id = std::env::var("PICOBOT_USER_ID").ok().unwrap_or_else(|| "local-user".to_string());
    let session_id = std::env::var("PICOBOT_SESSION_ID").ok().unwrap_or_else(|| "repl:local".to_string());
    let kernel = Arc::new(kernel.clone_with_context(Some(user_id), Some(session_id)));
    let agent = agent_builder.build(kernel.tool_registry(), kernel.clone(), config.max_turns());

    println!("picobot repl (type 'exit' to quit)");

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("> ");
        stdout.flush().context("failed to flush stdout")?;

        let mut input = String::new();
        stdin
            .read_line(&mut input)
            .context("failed to read stdin")?;
        let prompt = input.trim();
        if prompt.is_empty() {
            continue;
        }
        if prompt == "exit" {
            break;
        }

        let _response = match &agent {
            crate::providers::factory::ProviderAgent::OpenAI(inner) => {
                stream_prompt_to_stdout(inner, prompt, config.max_turns()).await
            }
            crate::providers::factory::ProviderAgent::OpenRouter(inner) => {
                stream_prompt_to_stdout(inner, prompt, config.max_turns()).await
            }
            crate::providers::factory::ProviderAgent::Gemini(inner) => {
                stream_prompt_to_stdout(inner, prompt, config.max_turns()).await
            }
        }
        .context("prompt failed")?;
    }

    Ok(())
}
