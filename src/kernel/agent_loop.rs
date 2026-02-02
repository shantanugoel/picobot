use crate::kernel::agent::Kernel;
use crate::models::traits::Model;
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

    pub fn last_user_message(&self) -> Option<&str> {
        self.messages.iter().rev().find_map(|message| match message {
            Message::User { content } => Some(content.as_str()),
            _ => None,
        })
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
                let content = if let Some(tool) = kernel.tool_registry().get(&call.name) {
                    match kernel.invoke_tool(tool, call.arguments).await {
                        Ok(result) => serde_json::to_string(&result)
                            .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?,
                        Err(err) => serde_json::to_string(&serde_json::json!({
                            "error": err.to_string()
                        }))
                        .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?,
                    }
                } else {
                    serde_json::to_string(&serde_json::json!({
                        "error": format!("unknown tool '{}'", call.name)
                    }))
                    .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?
                };
                state.push(Message::tool(call.id, content));
            }
            Ok(None)
        }
    }
}

pub enum AgentStep {
    AwaitUser,
    Responded(String),
}

pub async fn run_agent_step(
    kernel: &Kernel,
    model: &dyn Model,
    state: &mut ConversationState,
) -> Result<AgentStep, ToolError> {
    let request = build_model_request(state, kernel.tool_registry().tool_specs());
    let response = model
        .complete(request)
        .await
        .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;

    match handle_model_response(kernel, response, state).await? {
        Some(text) => Ok(AgentStep::Responded(text)),
        None => Ok(AgentStep::AwaitUser),
    }
}

pub async fn run_agent_step_streamed(
    kernel: &Kernel,
    model: &dyn Model,
    state: &mut ConversationState,
    on_token: &mut dyn FnMut(&str),
) -> Result<AgentStep, ToolError> {
    let request = build_model_request(state, kernel.tool_registry().tool_specs());
    let events = model
        .stream(request)
        .await
        .map_err(|err| ToolError::ExecutionFailed(err.to_string()))?;

    let mut content = String::new();
    let mut tool_calls = Vec::new();

    for event in events {
        match event {
            crate::models::types::ModelEvent::Token(token) => {
                on_token(&token);
                content.push_str(&token);
            }
            crate::models::types::ModelEvent::ToolCall(call) => {
                tool_calls.push(call);
            }
            crate::models::types::ModelEvent::Done(_) => {}
        }
    }

    let response = if !tool_calls.is_empty() {
        ModelResponse::ToolCalls(tool_calls)
    } else {
        ModelResponse::Text(content)
    };

    match handle_model_response(kernel, response, state).await? {
        Some(text) => Ok(AgentStep::Responded(text)),
        None => Ok(AgentStep::AwaitUser),
    }
}

pub async fn run_agent_loop(
    kernel: &Kernel,
    model: &dyn Model,
    state: &mut ConversationState,
    user_message: String,
) -> Result<String, ToolError> {
    run_agent_loop_with_limit(kernel, model, state, user_message, 8).await
}

pub async fn run_agent_loop_with_limit(
    kernel: &Kernel,
    model: &dyn Model,
    state: &mut ConversationState,
    user_message: String,
    max_tool_rounds: usize,
) -> Result<String, ToolError> {
    state.push(Message::user(user_message));
    for _ in 0..max_tool_rounds {
        match run_agent_step(kernel, model, state).await? {
            AgentStep::Responded(text) => return Ok(text),
            AgentStep::AwaitUser => continue,
        }
    }
    Err(ToolError::ExecutionFailed(
        "tool call loop exceeded limit".to_string(),
    ))
}

pub async fn run_agent_loop_streamed(
    kernel: &Kernel,
    model: &dyn Model,
    state: &mut ConversationState,
    user_message: String,
    on_token: &mut dyn FnMut(&str),
) -> Result<String, ToolError> {
    run_agent_loop_streamed_with_limit(kernel, model, state, user_message, on_token, 8).await
}

pub async fn run_agent_loop_streamed_with_limit(
    kernel: &Kernel,
    model: &dyn Model,
    state: &mut ConversationState,
    user_message: String,
    on_token: &mut dyn FnMut(&str),
    max_tool_rounds: usize,
) -> Result<String, ToolError> {
    state.push(Message::user(user_message));
    for _ in 0..max_tool_rounds {
        match run_agent_step_streamed(kernel, model, state, on_token).await? {
            AgentStep::Responded(text) => return Ok(text),
            AgentStep::AwaitUser => continue,
        }
    }
    Err(ToolError::ExecutionFailed(
        "tool call loop exceeded limit".to_string(),
    ))
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
    use async_trait::async_trait;
    use serde_json::json;

    use crate::kernel::agent::Kernel;
    use crate::kernel::agent_loop::{build_model_request, run_agent_loop, ConversationState};
    use crate::kernel::permissions::CapabilitySet;
    use crate::models::traits::{Model, ModelError};
    use crate::models::types::{Message, ModelEvent, ModelResponse, ToolInvocation};
    use crate::tools::registry::ToolRegistry;
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

    #[derive(Debug)]
    struct StaticModel;

    #[async_trait]
    impl Model for StaticModel {
        fn info(&self) -> crate::models::types::ModelInfo {
            crate::models::types::ModelInfo {
                id: "static".to_string(),
                provider: "test".to_string(),
                model: "static".to_string(),
            }
        }

        async fn complete(
            &self,
            _req: crate::models::types::ModelRequest,
        ) -> Result<ModelResponse, ModelError> {
            Ok(ModelResponse::Text("ok".to_string()))
        }

        async fn stream(
            &self,
            _req: crate::models::types::ModelRequest,
        ) -> Result<Vec<ModelEvent>, ModelError> {
            Ok(vec![ModelEvent::Done(ModelResponse::Text(
                "ok".to_string(),
            ))])
        }
    }

    #[tokio::test]
    async fn run_agent_loop_returns_response() {
        let registry = ToolRegistry::new();
        let kernel = Kernel::new(registry, std::path::PathBuf::from("/"))
            .with_capabilities(CapabilitySet::empty());
        let mut state = ConversationState::new();

        let response = run_agent_loop(&kernel, &StaticModel, &mut state, "hi".to_string())
            .await
            .unwrap();

        assert_eq!(response, "ok");
    }
}
