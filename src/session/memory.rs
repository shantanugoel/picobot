use serde_json::json;

use crate::config::MemoryConfig;
use crate::session::db::SqliteStore;
use crate::session::error::SessionDbError;
use crate::session::types::{MessageType, StoredMessage};

#[derive(Debug, Clone)]
pub struct MemoryRetriever {
    pub config: MemoryConfig,
    store: SqliteStore,
}

impl MemoryRetriever {
    pub fn new(config: MemoryConfig, store: SqliteStore) -> Self {
        Self { config, store }
    }

    pub fn build_context(
        &self,
        user_id: Option<&str>,
        session_id: Option<&str>,
        session_messages: &[StoredMessage],
    ) -> Vec<StoredMessage> {
        let mut output = Vec::new();
        let include_summary = self.config.include_summary_on_truncation.unwrap_or(true);
        if self.config.enable_user_memories.unwrap_or(true)
            && let Some(user_id) = user_id
            && let Ok(memories) = load_user_memories(&self.store, user_id, self.max_user_memories())
            && !memories.is_empty()
        {
            let mut lines = Vec::new();
            for (key, content) in memories {
                lines.push(format!("- {key}: {content}"));
            }
            let body = format!("User memories:\n{}", lines.join("\n"));
            output.push(StoredMessage {
                message_type: MessageType::System,
                content: body,
                tool_call_id: None,
                seq_order: 0,
                token_estimate: None,
            });
        }

        let max_messages = self.config.max_session_messages.unwrap_or(20);
        let count = session_messages.len();
        let start = count.saturating_sub(max_messages);
        if include_summary
            && start > 0
            && let Some(summary) = load_session_summary(&self.store, session_id)
        {
            output.push(StoredMessage {
                message_type: MessageType::System,
                content: format!("Session summary:\n{summary}"),
                tool_call_id: None,
                seq_order: 0,
                token_estimate: None,
            });
        }

        output.extend_from_slice(&session_messages[start..]);
        self.apply_budget(output)
    }

    fn max_user_memories(&self) -> usize {
        self.config.max_user_memories.unwrap_or(50)
    }

    fn apply_budget(&self, mut messages: Vec<StoredMessage>) -> Vec<StoredMessage> {
        let budget = self.config.context_budget_tokens.unwrap_or(4000) as usize;
        if budget == 0 {
            return messages;
        }
        while estimate_tokens(&messages) > budget {
            if messages.len() <= 1 {
                break;
            }
            if messages.len() > 1 {
                messages.remove(1);
            }
        }
        messages
    }

    pub fn to_prompt_snippet(messages: &[StoredMessage]) -> Option<String> {
        let mut lines = Vec::new();
        for message in messages {
            match message.message_type {
                MessageType::System => {
                    lines.push(format!("[system] {}", message.content));
                }
                MessageType::User => {
                    lines.push(format!("[user] {}", message.content));
                }
                MessageType::Assistant => {
                    lines.push(format!("[assistant] {}", message.content));
                }
                MessageType::AssistantToolCalls => {
                    let value = serde_json::from_str::<serde_json::Value>(&message.content)
                        .unwrap_or_else(|_| json!({"tool_calls": message.content}));
                    lines.push(format!("[assistant_tool_calls] {}", value));
                }
                MessageType::Tool => {
                    if let Some(id) = &message.tool_call_id {
                        lines.push(format!("[tool:{id}] {}", message.content));
                    } else {
                        lines.push(format!("[tool] {}", message.content));
                    }
                }
            }
        }
        if lines.is_empty() {
            None
        } else {
            Some(lines.join("\n"))
        }
    }
}

fn load_user_memories(
    store: &SqliteStore,
    user_id: &str,
    max_items: usize,
) -> Result<Vec<(String, String)>, SessionDbError> {
    store.with_connection(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT key, content FROM user_memories WHERE user_id = ?1 ORDER BY updated_at DESC LIMIT ?2",
            )
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![user_id, max_items as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        let mut items = Vec::new();
        for row in rows {
            items.push(row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?);
        }
        Ok(items)
    })
}

fn estimate_tokens(messages: &[StoredMessage]) -> usize {
    messages
        .iter()
        .map(|message| match message.message_type {
            MessageType::System
            | MessageType::User
            | MessageType::Assistant
            | MessageType::Tool
            | MessageType::AssistantToolCalls => message.content.len().div_ceil(4),
        })
        .sum()
}

fn load_session_summary(store: &SqliteStore, session_id: Option<&str>) -> Option<String> {
    let session_id = session_id?;
    store
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare("SELECT summary FROM session_summaries WHERE session_id = ?1")
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let mut rows = stmt
                .query([session_id])
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let row = rows
                .next()
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            if let Some(row) = row {
                let summary: String = row
                    .get(0)
                    .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
                Ok(Some(summary))
            } else {
                Ok(None)
            }
        })
        .ok()
        .flatten()
}
