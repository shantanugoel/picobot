use futures::StreamExt;
use genai::adapter::AdapterKind;
use genai::chat::{
    ChatMessage, ChatOptions, ChatRequest, ChatResponse, ChatStreamEvent, ChatStreamResponse, Tool,
    ToolCall, ToolResponse,
};
use genai::resolver::{AuthData, AuthResolver, Endpoint, ServiceTargetResolver};
use genai::{Client, ModelIden};

use crate::models::traits::{Model, ModelError};
use crate::models::types::{
    Message, ModelEvent, ModelInfo, ModelRequest, ModelResponse, ToolInvocation,
};
use crate::tools::traits::ToolSpec;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct GenaiModel {
    info: ModelInfo,
    client: Client,
    adapter_kind: AdapterKind,
}

impl GenaiModel {
    pub fn new(info: ModelInfo, client: Client, adapter_kind: AdapterKind) -> Self {
        Self {
            info,
            client,
            adapter_kind,
        }
    }
}

#[async_trait::async_trait]
impl Model for GenaiModel {
    fn info(&self) -> ModelInfo {
        self.info.clone()
    }

    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse, ModelError> {
        if std::env::var("PICOBOT_LOG_MODEL_TRANSPORT").as_deref() == Ok("1") {
            eprintln!(
                "Model transport: provider={} adapter={:?} model={}",
                self.info.provider, self.adapter_kind, self.info.model
            );
        }
        let chat_req = build_request(&req, self.adapter_kind);
        let options = chat_options_for_request(&req);
        let response = self
            .client
            .exec_chat(&self.info.model, chat_req, Some(&options))
            .await
            .map_err(|err| ModelError::RequestFailed(err.to_string()))?;
        chat_response_to_model_response(response)
    }

    async fn stream(&self, req: ModelRequest) -> Result<Vec<ModelEvent>, ModelError> {
        if std::env::var("PICOBOT_LOG_MODEL_TRANSPORT").as_deref() == Ok("1") {
            eprintln!(
                "Model transport: provider={} adapter={:?} model={}",
                self.info.provider, self.adapter_kind, self.info.model
            );
        }
        let chat_req = build_request(&req, self.adapter_kind);
        let options = chat_options_for_request(&req);
        let ChatStreamResponse { mut stream, .. } = self
            .client
            .exec_chat_stream(&self.info.model, chat_req, Some(&options))
            .await
            .map_err(|err| ModelError::RequestFailed(err.to_string()))?;

        let mut events = Vec::new();
        let mut content = String::new();
        let mut tool_calls: Vec<ToolCall> = Vec::new();

        while let Some(event) = stream.next().await {
            let event = event.map_err(|err| ModelError::RequestFailed(err.to_string()))?;
            match event {
                ChatStreamEvent::Chunk(chunk) => {
                    if !chunk.content.is_empty() {
                        content.push_str(&chunk.content);
                        events.push(ModelEvent::Token(chunk.content));
                    }
                }
                ChatStreamEvent::ToolCallChunk(chunk) => {
                    tool_calls.push(chunk.tool_call);
                }
                ChatStreamEvent::End(end) => {
                    if let Some(calls) = end.captured_into_tool_calls() {
                        tool_calls.extend(calls);
                    }
                }
                _ => {}
            }
        }

        if !tool_calls.is_empty() {
            let mut seen = std::collections::HashSet::new();
            let mut deduped = Vec::new();
            for call in tool_calls {
                let key = (
                    call.call_id.clone(),
                    call.fn_name.clone(),
                    call.fn_arguments.to_string(),
                );
                if seen.insert(key) {
                    deduped.push(call);
                }
            }
            let invocations = deduped
                .into_iter()
                .map(tool_call_to_invocation)
                .collect::<Result<Vec<_>, ModelError>>()?;
            for invocation in invocations.clone() {
                events.push(ModelEvent::ToolCall(invocation));
            }
            events.push(ModelEvent::Done(ModelResponse::ToolCalls(invocations)));
        } else {
            events.push(ModelEvent::Done(ModelResponse::Text(content)));
        }
        Ok(events)
    }
}

pub fn build_client(
    provider: &str,
    model: &str,
    api_key_env: Option<&str>,
    base_url: Option<&str>,
) -> Result<(Client, AdapterKind), ModelError> {
    let adapter_kind = adapter_kind_from_provider(provider)?;
    let auth_resolver = build_auth_resolver(provider, adapter_kind, api_key_env, model)?;
    let target_resolver = build_service_target_resolver(adapter_kind, model, base_url);

    let client = Client::builder()
        .with_auth_resolver(auth_resolver)
        .with_service_target_resolver(target_resolver)
        .build();

    Ok((client, adapter_kind))
}

fn build_auth_resolver(
    provider: &str,
    adapter_kind: AdapterKind,
    api_key_env: Option<&str>,
    _model: &str,
) -> Result<AuthResolver, ModelError> {
    let env_name = if adapter_kind == AdapterKind::Ollama {
        api_key_env.map(|value| value.to_string())
    } else {
        Some(
            api_key_env
                .map(|value| value.to_string())
                .unwrap_or_else(|| {
                    default_key_env_for_provider(provider, adapter_kind).to_string()
                }),
        )
    };
    let env_name = Arc::new(env_name);
    Ok(AuthResolver::from_resolver_fn(
        move |model_iden: ModelIden| {
            if model_iden.adapter_kind != adapter_kind {
                return Ok(None);
            }
            if let Some(env_name) = env_name.as_ref() {
                return Ok(Some(AuthData::from_env(env_name.clone())));
            }
            Ok(None)
        },
    ))
}

fn build_service_target_resolver(
    adapter_kind: AdapterKind,
    model: &str,
    base_url: Option<&str>,
) -> ServiceTargetResolver {
    let model_name = model.to_string();
    let base_url = base_url.map(|value| value.trim_end_matches('/').to_string());
    ServiceTargetResolver::from_resolver_fn(move |mut target: genai::ServiceTarget| {
        if target.model.adapter_kind != adapter_kind {
            return Ok(target);
        }
        if !model_name.is_empty() {
            target.model = target.model.from_name(model_name.clone());
        }
        if let Some(url) = &base_url {
            target.endpoint = Endpoint::from_owned(url.clone());
        }
        Ok(target)
    })
}

fn default_key_env_for_provider(provider: &str, adapter_kind: AdapterKind) -> &'static str {
    if provider.eq_ignore_ascii_case("openrouter") {
        return "OPENROUTER_API_KEY";
    }
    match adapter_kind {
        AdapterKind::OpenAI | AdapterKind::OpenAIResp => "OPENAI_API_KEY",
        AdapterKind::Anthropic => "ANTHROPIC_API_KEY",
        AdapterKind::Gemini => "GEMINI_API_KEY",
        AdapterKind::Groq => "GROQ_API_KEY",
        AdapterKind::Ollama => "OLLAMA_API_KEY",
        AdapterKind::Fireworks => "FIREWORKS_API_KEY",
        AdapterKind::Together => "TOGETHER_API_KEY",
        AdapterKind::Cohere => "COHERE_API_KEY",
        AdapterKind::Xai => "XAI_API_KEY",
        AdapterKind::DeepSeek => "DEEPSEEK_API_KEY",
        AdapterKind::Zai => "ZAI_API_KEY",
        AdapterKind::Mimo => "MIMO_API_KEY",
        AdapterKind::Nebius => "NEBIUS_API_KEY",
        AdapterKind::BigModel => "BIGMODEL_API_KEY",
    }
}

fn adapter_kind_from_provider(provider: &str) -> Result<AdapterKind, ModelError> {
    let provider = provider.to_lowercase();
    let kind = match provider.as_str() {
        "openai" | "openrouter" => AdapterKind::OpenAI,
        "openai_resp" => AdapterKind::OpenAIResp,
        "anthropic" => AdapterKind::Anthropic,
        "gemini" | "google" => AdapterKind::Gemini,
        "groq" => AdapterKind::Groq,
        "ollama" => AdapterKind::Ollama,
        "fireworks" => AdapterKind::Fireworks,
        "together" => AdapterKind::Together,
        "cohere" => AdapterKind::Cohere,
        "xai" => AdapterKind::Xai,
        "deepseek" => AdapterKind::DeepSeek,
        "zai" => AdapterKind::Zai,
        "mimo" => AdapterKind::Mimo,
        "nebius" => AdapterKind::Nebius,
        "bigmodel" => AdapterKind::BigModel,
        _ => {
            return Err(ModelError::RequestFailed(format!(
                "unsupported provider '{provider}'"
            )));
        }
    };
    Ok(kind)
}

fn build_request(req: &ModelRequest, adapter_kind: AdapterKind) -> ChatRequest {
    let mut chat_req = ChatRequest::new(req.messages.iter().map(to_chat_message).collect());
    if !req.tools.is_empty() {
        let tools = req
            .tools
            .iter()
            .map(|tool| to_tool(tool, adapter_kind))
            .collect::<Vec<_>>();
        chat_req = chat_req.with_tools(tools);
    }
    chat_req
}

fn to_chat_message(message: &Message) -> ChatMessage {
    match message {
        Message::System { content } => ChatMessage::system(content.clone()),
        Message::User { content } => ChatMessage::user(content.clone()),
        Message::Assistant { content } => ChatMessage::assistant(content.clone()),
        Message::AssistantToolCalls { tool_calls } => ChatMessage::from(
            tool_calls
                .iter()
                .map(|call| ToolCall {
                    call_id: call.id.clone(),
                    fn_name: call.name.clone(),
                    fn_arguments: call.arguments.clone(),
                    thought_signatures: None,
                })
                .collect::<Vec<_>>(),
        ),
        Message::Tool {
            tool_call_id,
            content,
        } => ChatMessage::from(ToolResponse::new(tool_call_id, content)),
    }
}

fn to_tool(tool: &ToolSpec, adapter_kind: AdapterKind) -> Tool {
    let mut result = Tool::new(tool.name.clone()).with_description(tool.description.clone());
    let schema = if adapter_kind == AdapterKind::Gemini {
        strip_additional_properties(&tool.schema)
    } else {
        tool.schema.clone()
    };
    result = result.with_schema(schema);
    result
}

fn strip_additional_properties(value: &serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut cleaned = serde_json::Map::new();
            for (key, value) in map {
                if key == "additionalProperties" {
                    continue;
                }
                cleaned.insert(key.clone(), strip_additional_properties(value));
            }
            serde_json::Value::Object(cleaned)
        }
        serde_json::Value::Array(values) => {
            serde_json::Value::Array(values.iter().map(strip_additional_properties).collect())
        }
        _ => value.clone(),
    }
}

fn chat_response_to_model_response(response: ChatResponse) -> Result<ModelResponse, ModelError> {
    let tool_calls = response.tool_calls();
    if !tool_calls.is_empty() {
        let invocations = tool_calls
            .into_iter()
            .cloned()
            .map(tool_call_to_invocation)
            .collect::<Result<Vec<_>, ModelError>>()?;
        return Ok(ModelResponse::ToolCalls(invocations));
    }

    let text = response.first_text().unwrap_or("").to_string();
    Ok(ModelResponse::Text(text))
}

fn tool_call_to_invocation(call: ToolCall) -> Result<ToolInvocation, ModelError> {
    Ok(ToolInvocation {
        id: call.call_id,
        name: call.fn_name,
        arguments: call.fn_arguments,
    })
}

fn chat_options_for_request(req: &ModelRequest) -> ChatOptions {
    let mut options = ChatOptions::default()
        .with_capture_content(true)
        .with_capture_tool_calls(true);
    if let Some(max_tokens) = req.max_tokens {
        options = options.with_max_tokens(max_tokens);
    }
    if let Some(temperature) = req.temperature {
        options = options.with_temperature(temperature as f64);
    }
    options
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_google_to_gemini() {
        let kind = adapter_kind_from_provider("google").unwrap();
        assert!(matches!(kind, AdapterKind::Gemini));
    }
}
