use async_trait::async_trait;
use serde_json::{Value, json};

use crate::kernel::context::ToolContext;
use crate::kernel::permissions::{MemoryScope, Permission};
use crate::session::db::SqliteStore;
use crate::session::error::SessionDbError;
use crate::tools::traits::{Tool, ToolError, ToolOutput};

#[derive(Debug, Clone)]
pub struct MemoryTool {
    store: SqliteStore,
}

impl MemoryTool {
    pub fn new(store: SqliteStore) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for MemoryTool {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn description(&self) -> &'static str {
        "Save, list, or delete user memories"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": { "type": "string", "enum": ["save", "list", "delete"] },
                "key": { "type": "string" },
                "content": { "type": "string" }
            },
            "additionalProperties": false
        })
    }

    fn required_permissions(
        &self,
        _ctx: &ToolContext,
        input: &Value,
    ) -> Result<Vec<Permission>, ToolError> {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing action".to_string()))?;
        match action {
            "list" => Ok(vec![Permission::MemoryRead {
                scope: MemoryScope::User,
            }]),
            "save" | "delete" => Ok(vec![Permission::MemoryWrite {
                scope: MemoryScope::User,
            }]),
            _ => Err(ToolError::InvalidInput("invalid action".to_string())),
        }
    }

    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput, ToolError> {
        let action = input
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("missing action".to_string()))?;
        let user_id = ctx
            .user_id
            .as_ref()
            .ok_or_else(|| ToolError::ExecutionFailed("missing user_id".to_string()))?;
        match action {
            "list" => list_memories(&self.store, user_id),
            "save" => {
                let key = input
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::InvalidInput("missing key".to_string()))?;
                let content = input
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::InvalidInput("missing content".to_string()))?;
                validate_key(key)?;
                save_memory(&self.store, ctx, user_id, key, content)
            }
            "delete" => {
                let key = input
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolError::InvalidInput("missing key".to_string()))?;
                validate_key(key)?;
                delete_memory(&self.store, user_id, key)
            }
            _ => Err(ToolError::InvalidInput("invalid action".to_string())),
        }
    }
}

fn validate_key(key: &str) -> Result<(), ToolError> {
    if key.len() > 64 {
        return Err(ToolError::InvalidInput("key too long".to_string()));
    }
    if key.starts_with("system_") || key.starts_with("internal_") {
        return Err(ToolError::InvalidInput(
            "key prefix is reserved".to_string(),
        ));
    }
    let mut chars = key.chars();
    let first = chars
        .next()
        .ok_or_else(|| ToolError::InvalidInput("key required".to_string()))?;
    if !first.is_ascii_lowercase() {
        return Err(ToolError::InvalidInput(
            "key must start with a lowercase letter".to_string(),
        ));
    }
    for ch in chars {
        if !(ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_') {
            return Err(ToolError::InvalidInput(
                "key must be lowercase alphanumeric or underscore".to_string(),
            ));
        }
    }
    Ok(())
}

fn list_memories(store: &SqliteStore, user_id: &str) -> Result<ToolOutput, ToolError> {
    store
        .with_connection(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT key, content, updated_at FROM user_memories WHERE user_id = ?1 ORDER BY updated_at DESC",
                )
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let rows = stmt
                .query_map([user_id], |row| {
                    Ok(json!({
                        "key": row.get::<_, String>(0)?,
                        "content": row.get::<_, String>(1)?,
                        "updated_at": row.get::<_, String>(2)?,
                    }))
                })
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let mut items = Vec::new();
            for row in rows {
                items.push(row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?);
            }
            Ok(json!({"memories": items}))
        })
        .map_err(|err| ToolError::ExecutionFailed(err.to_string()))
}

fn save_memory(
    store: &SqliteStore,
    ctx: &ToolContext,
    user_id: &str,
    key: &str,
    content: &str,
) -> Result<ToolOutput, ToolError> {
    store
        .with_connection(|conn| {
            let now = chrono::Utc::now().to_rfc3339();
            let session_id = ctx.session_id.as_deref();
            conn.execute(
                "INSERT INTO user_memories (user_id, key, content, source_session_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(user_id, key) DO UPDATE SET content = excluded.content, source_session_id = excluded.source_session_id, updated_at = excluded.updated_at",
                rusqlite::params![user_id, key, content, session_id, now, now],
            )
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            Ok(json!({"status": "saved", "key": key}))
        })
        .map_err(|err| ToolError::ExecutionFailed(err.to_string()))
}

fn delete_memory(store: &SqliteStore, user_id: &str, key: &str) -> Result<ToolOutput, ToolError> {
    store
        .with_connection(|conn| {
            let count = conn
                .execute(
                    "DELETE FROM user_memories WHERE user_id = ?1 AND key = ?2",
                    rusqlite::params![user_id, key],
                )
                .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            Ok(json!({"status": "deleted", "key": key, "removed": count}))
        })
        .map_err(|err| ToolError::ExecutionFailed(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::validate_key;

    #[test]
    fn validate_key_rejects_prefixes() {
        assert!(validate_key("system_test").is_err());
        assert!(validate_key("internal_value").is_err());
    }

    #[test]
    fn validate_key_accepts_simple_key() {
        assert!(validate_key("favorite_color").is_ok());
    }
}
