use std::io::{self, Write};
use std::sync::Arc;

use crate::channels::permissions::channel_profile;
use crate::config::Config;
use crate::kernel::core::Kernel;
use crate::kernel::permissions::{Permission, PermissionPrompter, PromptDecision};
use crate::providers::factory::ProviderAgentBuilder;
use anyhow::{Context, Result};
use futures::StreamExt;
use rig::agent::{Agent, MultiTurnStreamItem, Text};
use rig::completion::{CompletionModel, GetTokenUsage};
use rig::streaming::{StreamedAssistantContent, StreamedUserContent, StreamingPrompt};
use rig::wasm_compat::WasmCompatSend;

use crate::session::manager::SessionManager;
use crate::session::memory::MemoryRetriever;
use crate::session::types::{MessageType, StoredMessage};
use async_trait::async_trait;

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

struct ReplPrompter;

#[async_trait]
impl PermissionPrompter for ReplPrompter {
    async fn prompt(
        &self,
        tool_name: &str,
        permissions: &[Permission],
        timeout_secs: u64,
    ) -> Option<PromptDecision> {
        println!("\nPermission required for tool '{tool_name}':");
        for permission in permissions {
            println!("- {permission}");
        }
        print!("Allow? [o]nce / [s]ession / [n]o (timeout {timeout_secs}s): ");
        let _ = io::stdout().flush();
        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            return None;
        }
        match input.trim().to_ascii_lowercase().as_str() {
            "o" | "once" => Some(PromptDecision::AllowOnce),
            "s" | "session" => Some(PromptDecision::AllowSession),
            "n" | "no" => Some(PromptDecision::Deny),
            _ => None,
        }
    }
}

pub async fn run(
    config: Config,
    kernel: Kernel,
    agent_builder: ProviderAgentBuilder,
) -> Result<()> {
    let user_id = std::env::var("PICOBOT_USER_ID")
        .ok()
        .unwrap_or_else(|| "local-user".to_string());
    let session_id = std::env::var("PICOBOT_SESSION_ID")
        .ok()
        .unwrap_or_else(|| "repl:local".to_string());
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let channel_id = "repl".to_string();
    let profile = channel_profile(&config.channels(), &channel_id, &base_dir);
    let kernel = Arc::new(
        kernel
            .clone_with_context(Some(user_id), Some(session_id))
            .with_channel_id(Some(channel_id))
            .with_prompt_profile(profile)
            .with_prompter(Some(Arc::new(ReplPrompter))),
    );
    let session_store = crate::session::db::SqliteStore::new(
        config
            .data_dir()
            .join("sessions.db")
            .to_string_lossy()
            .to_string(),
    );
    session_store.touch()?;
    let memory_config = config.memory();
    let session_manager = SessionManager::new(session_store.clone());
    let memory_retriever = MemoryRetriever::new(memory_config.clone(), session_store);
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

        let session_id = kernel
            .context()
            .session_id
            .clone()
            .unwrap_or_else(|| "repl:local".to_string());
        let session = match session_manager.get_session(&session_id)? {
            Some(session) => session,
            None => session_manager.create_session(
                session_id,
                "repl".to_string(),
                kernel
                    .context()
                    .channel_id
                    .clone()
                    .unwrap_or_else(|| "repl".to_string()),
                kernel
                    .context()
                    .user_id
                    .clone()
                    .unwrap_or_else(|| "local-user".to_string()),
                kernel.context().capabilities.as_ref().clone(),
            )?,
        };

        let existing_messages = session_manager
            .get_messages(
                &session.id,
                memory_config.max_session_messages.unwrap_or(50),
            )
            .unwrap_or_default();
        let filtered_messages = if memory_config.include_tool_messages() {
            existing_messages
        } else {
            existing_messages
                .into_iter()
                .filter(|message| message.message_type != MessageType::Tool)
                .collect::<Vec<_>>()
        };
        let context_messages = memory_retriever.build_context(
            kernel.context().user_id.as_deref(),
            kernel.context().session_id.as_deref(),
            &filtered_messages,
        );
        let context_snippet = MemoryRetriever::to_prompt_snippet(&context_messages);
        let prompt_to_send = if let Some(context) = context_snippet {
            format!("Context:\n{context}\n\nUser: {prompt}")
        } else {
            prompt.to_string()
        };

        let mut seq_order = match session_manager.get_messages(&session.id, 1) {
            Ok(messages) => messages
                .last()
                .map(|message| message.seq_order + 1)
                .unwrap_or(0),
            Err(_) => 0,
        };

        let user_message = StoredMessage {
            message_type: MessageType::User,
            content: prompt.to_string(),
            tool_call_id: None,
            seq_order,
            token_estimate: None,
        };
        match session_manager.append_message(&session.id, &user_message) {
            Ok(()) => seq_order += 1,
            Err(err) => {
                tracing::warn!(error = %err, "failed to store user message");
            }
        }

        let response = match &agent {
            crate::providers::factory::ProviderAgent::OpenAI(inner) => {
                stream_prompt_to_stdout(inner, &prompt_to_send, config.max_turns()).await
            }
            crate::providers::factory::ProviderAgent::OpenRouter(inner) => {
                stream_prompt_to_stdout(inner, &prompt_to_send, config.max_turns()).await
            }
            crate::providers::factory::ProviderAgent::Gemini(inner) => {
                stream_prompt_to_stdout(inner, &prompt_to_send, config.max_turns()).await
            }
        };
        let response = match response {
            Ok(response) => response,
            Err(err) => {
                tracing::error!(error = %err, "prompt failed");
                println!("Sorry, something went wrong: {err}");
                continue;
            }
        };

        let assistant_message = StoredMessage {
            message_type: MessageType::Assistant,
            content: response,
            tool_call_id: None,
            seq_order,
            token_estimate: None,
        };
        if let Err(err) = session_manager.append_message(&session.id, &assistant_message) {
            tracing::warn!(error = %err, "failed to store assistant message");
        }
        if let Err(err) = session_manager.touch(&session.id) {
            tracing::warn!(error = %err, "failed to update session activity");
        }
    }

    Ok(())
}
