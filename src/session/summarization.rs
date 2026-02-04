use crate::models::traits::Model;
use crate::models::types::{Message, ModelRequest, ModelResponse};
use crate::session::db::SqliteStore;
use crate::session::error::{SessionDbError, SessionDbResult};

pub async fn summarize_session(
    store: &SqliteStore,
    model: &dyn Model,
    session_id: &str,
    message_count: usize,
) -> SessionDbResult<Option<String>> {
    if message_count == 0 {
        return Ok(None);
    }
    let messages = load_session_messages(store, session_id, message_count)?;
    if messages.is_empty() {
        return Ok(None);
    }
    let prompt = format!(
        "Summarize the conversation in 6-10 bullet points focused on user intent, decisions, and outcomes.\n\n{}",
        render_messages(&messages)
    );
    let request = ModelRequest {
        messages: vec![Message::system(prompt)],
        tools: Vec::new(),
        max_tokens: None,
        temperature: Some(0.2),
    };
    let response = model
        .complete(request)
        .await
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
    let summary = match response {
        ModelResponse::Text(text) => text,
        ModelResponse::ToolCalls(_) => "".to_string(),
    };
    if summary.trim().is_empty() {
        return Ok(None);
    }
    store_summary(store, session_id, summary.clone(), message_count)?;
    Ok(Some(summary))
}

fn load_session_messages(
    store: &SqliteStore,
    session_id: &str,
    max_items: usize,
) -> SessionDbResult<Vec<Message>> {
    store.with_connection(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT message_type, content, tool_call_id FROM messages
                 WHERE session_id = ?1 ORDER BY seq_order DESC LIMIT ?2",
            )
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        let rows = stmt
            .query_map(rusqlite::params![session_id, max_items as i64], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        let mut items = Vec::new();
        for row in rows {
            let (message_type, content, tool_call_id) =
                row.map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
            let message = match message_type.as_str() {
                "system" => Message::system(content),
                "user" => Message::user(content),
                "assistant" => {
                    match serde_json::from_str::<Vec<crate::models::types::ToolInvocation>>(
                        &content,
                    ) {
                        Ok(tool_calls) => Message::assistant_tool_calls(tool_calls),
                        Err(_) => Message::assistant(content),
                    }
                }
                "tool" => Message::tool(
                    tool_call_id.unwrap_or_else(|| "unknown".to_string()),
                    content,
                ),
                _ => Message::assistant(content),
            };
            items.push(message);
        }
        items.reverse();
        Ok(items)
    })
}

fn store_summary(
    store: &SqliteStore,
    session_id: &str,
    summary: String,
    message_count: usize,
) -> SessionDbResult<()> {
    store.with_connection(|conn| {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO session_summaries (session_id, summary, message_count, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id) DO UPDATE SET summary = excluded.summary, message_count = excluded.message_count, updated_at = excluded.updated_at",
            rusqlite::params![session_id, summary, message_count as i64, now, now],
        )
        .map_err(|err| SessionDbError::QueryFailed(err.to_string()))?;
        Ok(())
    })
}

fn render_messages(messages: &[Message]) -> String {
    let mut output = String::new();
    for message in messages {
        let (label, content) = match message {
            Message::System { content } => ("System", content.clone()),
            Message::User { content } => ("User", content.clone()),
            Message::Assistant { content } => ("Assistant", content.clone()),
            Message::AssistantToolCalls { tool_calls } => {
                let names = tool_calls
                    .iter()
                    .map(|call| call.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                ("Assistant", format!("tool calls: {names}"))
            }
            Message::Tool { content, .. } => ("Tool", content.clone()),
        };
        output.push_str(label);
        output.push_str(": ");
        output.push_str(&content);
        output.push('\n');
    }
    output
}
