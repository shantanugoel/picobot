use crate::kernel::agent::Kernel;
use crate::models::types::{Message, ModelRequest, ModelResponse};
use crate::tools::traits::{ToolError, ToolSpec};

pub struct ConversationState {
    messages: Vec<Message>,
}

impl ConversationState {
    pub fn new() -> Self {
        Self { messages: Vec::new() }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }
}

impl Default for ConversationState {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn handle_model_response(
    kernel: &Kernel,
    response: ModelResponse,
    state: &mut ConversationState,
) -> Result<Option<String>, ToolError> {
    match response {
        ModelResponse::Text(text) => {
            state.push(Message::assistant(text.clone()));
            Ok(Some(text))
        }
        ModelResponse::ToolCalls(calls) => {
            for call in calls {
                let tool = kernel
                    .tool_registry()
                    .get(&call.name)
                    .ok_or_else(|| ToolError::ExecutionFailed("unknown tool".to_string()))?;
                let result = kernel.invoke_tool(tool, call.arguments).await?;
                let content = serde_json::to_string(&result)
                    .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;
                state.push(Message::tool(call.id, content));
            }
            Ok(None)
        }
    }
}

pub fn build_model_request(state: &ConversationState, tools: Vec<ToolSpec>) -> ModelRequest {
    ModelRequest {
        messages: state.messages().to_vec(),
        tools,
        max_tokens: None,
        temperature: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::kernel::agent_loop::{build_model_request, ConversationState};
    use crate::models::types::{Message, ModelResponse, ToolInvocation};
    use crate::tools::traits::ToolSpec;

    #[test]
    fn build_model_request_copies_messages() {
        let mut state = ConversationState::new();
        state.push(Message::user("hello"));

        let request = build_model_request(&state, Vec::new());
        assert_eq!(request.messages.len(), 1);
    }

    #[test]
    fn conversation_state_appends_messages() {
        let mut state = ConversationState::new();
        state.push(Message::assistant("hi"));
        assert_eq!(state.messages().len(), 1);
    }

    #[test]
    fn tool_specs_can_be_attached() {
        let mut state = ConversationState::new();
        state.push(Message::user("ping"));
        let tools = vec![ToolSpec {
            name: "echo".to_string(),
            description: "echo tool".to_string(),
            schema: json!({"type": "object"}),
        }];
        let request = build_model_request(&state, tools);
        assert_eq!(request.tools.len(), 1);
    }

    #[test]
    fn model_response_text_is_representable() {
        let response = ModelResponse::Text("ok".to_string());
        match response {
            ModelResponse::Text(text) => assert_eq!(text, "ok"),
            _ => panic!("unexpected response"),
        }
    }

    #[test]
    fn model_response_tool_calls_is_representable() {
        let response = ModelResponse::ToolCalls(vec![ToolInvocation {
            id: "1".to_string(),
            name: "echo".to_string(),
            arguments: json!({"value": "ok"}),
        }]);
        match response {
            ModelResponse::ToolCalls(calls) => assert_eq!(calls.len(), 1),
            _ => panic!("unexpected response"),
        }
    }
}
