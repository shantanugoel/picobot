use crate::config::MemoryConfig;
use crate::kernel::context::ToolContext;
use crate::kernel::permissions::{MemoryScope, Permission};
use crate::models::types::Message;
use crate::session::db::SqliteStore;
use crate::session::error::SessionDbError;

#[derive(Debug, Clone)]
pub struct MemoryRetriever {
    pub config: MemoryConfig,
    store: SqliteStore,
}

impl MemoryRetriever {
    pub fn new(config: MemoryConfig, store: SqliteStore) -> Self {
        Self { config, store }
    }

    pub fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::MemoryRead {
            scope: MemoryScope::User,
        }]
    }

    pub fn build_context(&self, ctx: &ToolContext, session_messages: &[Message]) -> Vec<Message> {
        let mut output = Vec::new();
        if let Some(user_id) = ctx.user_id.as_ref()
            && let Ok(memories) =
                load_user_memories(&self.store, user_id, self.max_user_memories())
            && !memories.is_empty()
        {
            let mut lines = Vec::new();
            for (key, content) in memories {
                lines.push(format!("- {key}: {content}"));
            }
            let body = format!("User memories:\n{}", lines.join("\n"));
            output.push(Message::system(body));
        }

        let max_messages = self.config.max_session_messages.unwrap_or(20);
        let count = session_messages.len();
        let start = count.saturating_sub(max_messages);
        output.extend_from_slice(&session_messages[start..]);

        self.apply_budget(output)
    }

    fn max_user_memories(&self) -> usize {
        self.config.max_user_memories.unwrap_or(50)
    }

    fn apply_budget(&self, mut messages: Vec<Message>) -> Vec<Message> {
        let budget = self.config.context_budget_tokens.unwrap_or(4000) as usize;
        if budget == 0 {
            return messages;
        }
        while estimate_tokens(&messages) > budget {
            if messages.len() <= 1 {
                break;
            }
            messages.remove(1.min(messages.len() - 1));
        }
        messages
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

fn estimate_tokens(messages: &[Message]) -> usize {
    messages
        .iter()
        .map(|message| match message {
            Message::System { content }
            | Message::User { content }
            | Message::Assistant { content }
            | Message::Tool { content, .. } => content.len().div_ceil(4),
        })
        .sum()
}
