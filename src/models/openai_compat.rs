use async_openai::Client;
use async_openai::config::OpenAIConfig;
use async_openai::types::chat::{
    ChatCompletionMessageToolCallChunk, ChatCompletionMessageToolCalls,
    ChatCompletionRequestAssistantMessageArgs, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessageArgs, ChatCompletionRequestToolMessageArgs,
    ChatCompletionRequestUserMessageArgs, ChatCompletionTool, ChatCompletionTools,
    CreateChatCompletionRequest, CreateChatCompletionRequestArgs, FunctionObjectArgs,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{Value, json};

use crate::models::traits::{Model, ModelError};
use crate::models::types::{
    Message, ModelEvent, ModelInfo, ModelRequest, ModelResponse, ToolInvocation,
};
use crate::tools::traits::ToolSpec;

#[derive(Debug, Clone)]
pub struct OpenAICompatModel {
    info: ModelInfo,
    client: Client<OpenAIConfig>,
}

impl OpenAICompatModel {
    pub fn new(info: ModelInfo, client: Client<OpenAIConfig>) -> Self {
        Self { info, client }
    }
}

#[async_trait]
impl Model for OpenAICompatModel {
    fn info(&self) -> ModelInfo {
        self.info.clone()
    }

    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
        let request = build_request(&self.info, &req)?;
        let response = self
            .client
            .chat()
            .create(request)
            .await
            .map_err(|err| ModelError::RequestFailed(err.to_string()))?;

        let choice = response
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ModelError::InvalidResponse("missing choices".to_string()))?;

        let message = choice.message;
        if let Some(tool_calls) = message.tool_calls {
            let mut invocations = Vec::new();
            for tool_call in tool_calls {
                if let Some(invocation) = tool_call_to_invocation(tool_call)? {
                    invocations.push(invocation);
                }
            }
            return Ok(ModelResponse::ToolCalls(invocations));
        }

        let content = message.content.unwrap_or_else(|| "".to_string());
        Ok(ModelResponse::Text(content))
    }

    async fn stream(&self, req: ModelRequest) -> Result<Vec<ModelEvent>, ModelError> {
        let provider = self.info.provider.to_lowercase();
        if provider == "google" || provider == "gemini" {
            let response = self.complete(req).await?;
            let mut events = Vec::new();
            match response {
                ModelResponse::Text(content) => {
                    if !content.is_empty() {
                        events.push(ModelEvent::Token(content));
                    }
                    events.push(ModelEvent::Done(ModelResponse::Text(String::new())));
                }
                ModelResponse::ToolCalls(invocations) => {
                    for invocation in invocations.clone() {
                        events.push(ModelEvent::ToolCall(invocation));
                    }
                    events.push(ModelEvent::Done(ModelResponse::ToolCalls(invocations)));
                }
            }
            return Ok(events);
        }

        let mut request = build_request(&self.info, &req)?;
        request.stream = Some(true);

        let mut stream = self
            .client
            .chat()
            .create_stream(request)
            .await
            .map_err(|err| ModelError::RequestFailed(err.to_string()))?;

        let mut events = Vec::new();
        let mut tool_call_accumulator: Vec<ToolCallAccumulator> = Vec::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|err| ModelError::RequestFailed(err.to_string()))?;
            for choice in chunk.choices {
                if let Some(delta) = choice.delta.content
                    && !delta.is_empty()
                {
                    events.push(ModelEvent::Token(delta));
                }

                if let Some(tool_calls) = choice.delta.tool_calls {
                    apply_tool_call_deltas(&mut tool_call_accumulator, tool_calls);
                }

                if choice.finish_reason.is_some() && !tool_call_accumulator.is_empty() {
                    let invocations = tool_call_accumulator
                        .drain(..)
                        .map(ToolCallAccumulator::into_invocation)
                        .collect::<Result<Vec<_>, ModelError>>()?;
                    for invocation in invocations {
                        events.push(ModelEvent::ToolCall(invocation));
                    }
                }
            }
        }

        let response = if events
            .iter()
            .any(|event| matches!(event, ModelEvent::ToolCall(_)))
        {
            let invocations = events
                .iter()
                .filter_map(|event| match event {
                    ModelEvent::ToolCall(invocation) => Some(invocation.clone()),
                    _ => None,
                })
                .collect();
            ModelResponse::ToolCalls(invocations)
        } else {
            let mut content = String::new();
            for event in &events {
                if let ModelEvent::Token(token) = event {
                    content.push_str(token);
                }
            }
            ModelResponse::Text(content)
        };

        events.push(ModelEvent::Done(response));
        Ok(events)
    }
}

fn build_request(
    info: &ModelInfo,
    req: &ModelRequest,
) -> Result<CreateChatCompletionRequest, ModelError> {
    let provider = info.provider.to_lowercase();
    let messages = req
        .messages
        .iter()
        .map(|message| to_chat_message(message, &provider))
        .collect::<Result<Vec<_>, ModelError>>()?;
    let tools = to_tools(&req.tools)?;

    let mut builder = CreateChatCompletionRequestArgs::default();
    builder.model(info.model.clone());
    builder.messages(messages);
    if !tools.is_empty() {
        builder.tools(tools);
    }
    if let Some(max_tokens) = req.max_tokens {
        builder.max_tokens(max_tokens);
    }
    if let Some(temperature) = req.temperature {
        builder.temperature(temperature);
    }

    builder
        .build()
        .map_err(|err| ModelError::RequestFailed(err.to_string()))
}

fn to_chat_message(
    message: &Message,
    provider: &str,
) -> Result<ChatCompletionRequestMessage, ModelError> {
    match message {
        Message::System { content } => Ok(ChatCompletionRequestSystemMessageArgs::default()
            .content(content.clone())
            .build()
            .map_err(|err| ModelError::InvalidResponse(err.to_string()))?
            .into()),
        Message::User { content } => Ok(ChatCompletionRequestUserMessageArgs::default()
            .content(content.clone())
            .build()
            .map_err(|err| ModelError::InvalidResponse(err.to_string()))?
            .into()),
        Message::Assistant { content } => Ok(ChatCompletionRequestAssistantMessageArgs::default()
            .content(content.clone())
            .build()
            .map_err(|err| ModelError::InvalidResponse(err.to_string()))?
            .into()),
        Message::Tool {
            tool_call_id,
            content,
        } => {
            if provider == "google" || provider == "gemini" {
                return Ok(ChatCompletionRequestAssistantMessageArgs::default()
                    .content(content.clone())
                    .build()
                    .map_err(|err| ModelError::InvalidResponse(err.to_string()))?
                    .into());
            }
            Ok(ChatCompletionRequestToolMessageArgs::default()
                .tool_call_id(tool_call_id)
                .content(content.clone())
                .build()
                .map_err(|err| ModelError::InvalidResponse(err.to_string()))?
                .into())
        }
    }
}

fn to_tools(tools: &[ToolSpec]) -> Result<Vec<ChatCompletionTools>, ModelError> {
    let mut result = Vec::with_capacity(tools.len());
    for tool in tools {
        let function = FunctionObjectArgs::default()
            .name(tool.name.clone())
            .description(tool.description.clone())
            .parameters(tool.schema.clone())
            .build()
            .map_err(|err| ModelError::InvalidResponse(err.to_string()))?;
        result.push(ChatCompletionTools::Function(ChatCompletionTool {
            function,
        }));
    }
    Ok(result)
}

fn tool_call_to_invocation(
    call: ChatCompletionMessageToolCalls,
) -> Result<Option<ToolInvocation>, ModelError> {
    match call {
        ChatCompletionMessageToolCalls::Function(function_call) => {
            let arguments: Value = serde_json::from_str(&function_call.function.arguments)
                .unwrap_or_else(|_| json!({"raw": function_call.function.arguments}));
            Ok(Some(ToolInvocation {
                id: function_call.id,
                name: function_call.function.name,
                arguments,
            }))
        }
        ChatCompletionMessageToolCalls::Custom(_) => Ok(None),
    }
}

#[derive(Debug, Default)]
struct ToolCallAccumulator {
    id: String,
    name: String,
    arguments: String,
}

impl ToolCallAccumulator {
    fn into_invocation(self) -> Result<ToolInvocation, ModelError> {
        let arguments: Value = serde_json::from_str(&self.arguments)
            .unwrap_or_else(|_| json!({"raw": self.arguments}));
        Ok(ToolInvocation {
            id: self.id,
            name: self.name,
            arguments,
        })
    }
}

fn apply_tool_call_deltas(
    accumulators: &mut Vec<ToolCallAccumulator>,
    deltas: Vec<ChatCompletionMessageToolCallChunk>,
) {
    for delta in deltas {
        let index = delta.index as usize;
        if accumulators.len() <= index {
            accumulators.resize_with(index + 1, ToolCallAccumulator::default);
        }

        let accumulator = &mut accumulators[index];
        if let Some(id) = delta.id {
            accumulator.id = id;
        }
        if let Some(function) = delta.function {
            if let Some(name) = function.name {
                accumulator.name = name;
            }
            if let Some(arguments) = function.arguments {
                accumulator.arguments.push_str(&arguments);
            }
        }
    }
}
