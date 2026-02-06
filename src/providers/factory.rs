use std::env;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use rig::agent::Agent;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::providers::{gemini, openai, openrouter};
use rig::tool::ToolDyn;
use tokio::time::sleep;

use crate::config::{Config, ModelConfig};
use crate::kernel::core::Kernel;
use crate::providers::error::ProviderError;
use crate::tools::registry::ToolRegistry;
use crate::tools::rig_wrapper::KernelBackedTool;

pub const DEFAULT_PROVIDER_RETRIES: usize = 2;

#[derive(Debug, Clone, Copy)]
pub enum ProviderKind {
    OpenAI,
    OpenRouter,
    Gemini,
}

impl std::str::FromStr for ProviderKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value.trim().to_lowercase().as_str() {
            "openai" => Ok(Self::OpenAI),
            "openrouter" => Ok(Self::OpenRouter),
            "gemini" => Ok(Self::Gemini),
            other => Err(anyhow::anyhow!("unsupported provider '{other}'")),
        }
    }
}

pub struct ProviderFactory;

impl ProviderFactory {
    #[allow(dead_code)]
    pub fn build_openai_client(config: &Config) -> Result<openai::Client> {
        let api_key_env = config.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
        let api_key = env::var(api_key_env)
            .with_context(|| format!("missing API key in env '{api_key_env}'"))?;

        let mut builder = openai::Client::builder().api_key(api_key);
        if let Some(base_url) = &config.base_url {
            builder = builder.base_url(base_url);
        }

        builder.build().context("failed to build OpenAI client")
    }

    #[allow(dead_code)]
    pub fn build_openrouter_client(config: &Config) -> Result<openrouter::Client> {
        let api_key_env = config
            .api_key_env
            .as_deref()
            .unwrap_or("OPENROUTER_API_KEY");
        let api_key = env::var(api_key_env)
            .with_context(|| format!("missing API key in env '{api_key_env}'"))?;
        Ok(openrouter::Client::new(api_key)?)
    }

    #[allow(dead_code)]
    pub fn build_gemini_client(config: &Config) -> Result<gemini::Client> {
        let api_key_env = config.api_key_env.as_deref().unwrap_or("GEMINI_API_KEY");
        let api_key = env::var(api_key_env)
            .with_context(|| format!("missing API key in env '{api_key_env}'"))?;
        Ok(gemini::Client::builder().api_key(api_key).build()?)
    }

    #[allow(dead_code)]
    pub fn build_agent(
        config: &Config,
        tool_registry: &ToolRegistry,
        kernel: Arc<Kernel>,
    ) -> Result<ProviderAgent> {
        let router = Self::build_agent_router(config)?;
        router.build_default(config, tool_registry, kernel, config.max_turns())
    }

    pub fn build_agent_builder(config: &Config) -> Result<ProviderAgentBuilder> {
        ProviderAgentBuilder::new(config)
    }

    pub fn build_agent_router(config: &Config) -> Result<ModelRouter> {
        ModelRouter::new(config)
    }

    pub fn build_multimodal_agent(config: &Config) -> Result<ProviderAgent> {
        let multimodal = config
            .multimodal
            .clone()
            .or_else(|| config.vision.clone().map(crate::config::MultimodalConfig::from));
        let Some(multimodal) = multimodal else {
            let fallback = ProviderAgentBuilder::new(config)?;
            return fallback.build_without_tools();
        };

        if let Some(model_id) = &multimodal.model_id {
            let router = ModelRouter::new(config)?;
            if router.models.is_empty() {
                return Err(anyhow::anyhow!(
                    "multimodal.model_id '{model_id}' requires [[models]]"
                ));
            }
            let model = router
                .models
                .iter()
                .find(|model| model.id == *model_id)
                .ok_or_else(|| anyhow::anyhow!("multimodal.model_id '{model_id}' not found"))?;
            let mut builder = ProviderAgentBuilder::from_model_config(model, config)?;
            builder.system_prompt = multimodal
                .system_prompt
                .clone()
                .unwrap_or_else(|| config.system_prompt().to_string());
            if multimodal.base_url.is_some() {
                builder.base_url = multimodal.base_url.clone();
            }
            if multimodal.api_key_env.is_some() {
                builder.api_key_env = multimodal.api_key_env.clone();
            }
            return builder.build_without_tools();
        }

        let provider = multimodal.provider.as_deref().unwrap_or_else(|| config.provider());
        let model = multimodal
            .model
            .clone()
            .unwrap_or_else(|| config.model().to_string());
        let system_prompt = multimodal
            .system_prompt
            .clone()
            .unwrap_or_else(|| config.system_prompt().to_string());
        let base_url = multimodal.base_url.clone().or_else(|| config.base_url.clone());
        let api_key_env = multimodal
            .api_key_env
            .clone()
            .or_else(|| config.api_key_env.clone());
        let builder = ProviderAgentBuilder::from_parts(
            provider.parse()?,
            model,
            system_prompt,
            base_url,
            api_key_env,
        );
        builder.build_without_tools()
    }
}

fn build_agent_with_tools<M>(
    builder: rig::agent::AgentBuilder<M>,
    tool_registry: &ToolRegistry,
    kernel: Arc<Kernel>,
    max_turns: usize,
) -> Agent<M>
where
    M: rig::completion::CompletionModel,
{
    let builder = builder.default_max_turns(max_turns);
    let tools = tool_registry
        .specs()
        .into_iter()
        .map(|spec| {
            let wrapped = KernelBackedTool::new(spec, kernel.clone());
            Box::new(wrapped) as Box<dyn ToolDyn>
        })
        .collect::<Vec<_>>();

    if tools.is_empty() {
        builder.build()
    } else {
        builder.tools(tools).build()
    }
}

#[derive(Clone)]
pub struct ProviderAgentBuilder {
    provider: ProviderKind,
    model: String,
    system_prompt: String,
    base_url: Option<String>,
    api_key_env: Option<String>,
}

impl ProviderAgentBuilder {
    pub fn new(config: &Config) -> Result<Self> {
        Ok(Self {
            provider: config.provider().parse()?,
            model: config.model().to_string(),
            system_prompt: config.system_prompt().to_string(),
            base_url: config.base_url.clone(),
            api_key_env: config.api_key_env.clone(),
        })
    }

    pub fn from_model_config(model: &ModelConfig, fallback: &Config) -> Result<Self> {
        let provider = model
            .provider
            .as_deref()
            .unwrap_or_else(|| fallback.provider());
        Ok(Self {
            provider: provider.parse()?,
            model: model.model.clone(),
            system_prompt: model
                .system_prompt
                .clone()
                .unwrap_or_else(|| fallback.system_prompt().to_string()),
            base_url: model.base_url.clone().or_else(|| fallback.base_url.clone()),
            api_key_env: model
                .api_key_env
                .clone()
                .or_else(|| fallback.api_key_env.clone()),
        })
    }

    #[allow(dead_code)]
    pub fn from_parts(
        provider: ProviderKind,
        model: String,
        system_prompt: String,
        base_url: Option<String>,
        api_key_env: Option<String>,
    ) -> Self {
        Self {
            provider,
            model,
            system_prompt,
            base_url,
            api_key_env,
        }
    }
}

#[derive(Clone)]
pub struct ModelRouter {
    models: Vec<ModelConfig>,
    default_id: Option<String>,
}

impl ModelRouter {
    pub fn new(config: &Config) -> Result<Self> {
        let models = config.models.clone().unwrap_or_default();
        if models.is_empty() {
            return Ok(Self {
                models: Vec::new(),
                default_id: None,
            });
        }
        let mut seen = std::collections::HashSet::new();
        for model in &models {
            if model.id.trim().is_empty() {
                return Err(anyhow::anyhow!("model id cannot be empty"));
            }
            if !seen.insert(model.id.clone()) {
                return Err(anyhow::anyhow!("duplicate model id '{}'", model.id));
            }
        }
        let default_id = match config.default_model_id() {
            Some(id) if models.iter().any(|model| model.id == id) => Some(id.to_string()),
            Some(_) => Some(models[0].id.clone()),
            None => Some(models[0].id.clone()),
        };
        Ok(Self { models, default_id })
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    pub fn build_default(
        &self,
        fallback: &Config,
        tool_registry: &ToolRegistry,
        kernel: Arc<Kernel>,
        max_turns: usize,
    ) -> Result<ProviderAgent> {
        if self.models.is_empty() {
            let fallback = ProviderAgentBuilder::new(fallback)?;
            return fallback.build(tool_registry, kernel, max_turns);
        }
        let model = if let Some(default_id) = &self.default_id {
            self.models
                .iter()
                .find(|model| model.id == *default_id)
                .unwrap_or(&self.models[0])
        } else {
            &self.models[0]
        };
        let max_turns = model.max_turns.unwrap_or(max_turns);
        let builder = ProviderAgentBuilder::from_model_config(model, fallback)?;
        builder.build(tool_registry, kernel, max_turns)
    }
}

impl ProviderAgentBuilder {
    pub fn build(
        self,
        tool_registry: &ToolRegistry,
        kernel: Arc<Kernel>,
        max_turns: usize,
    ) -> Result<ProviderAgent> {
        self.build_with_env(tool_registry, kernel, max_turns, |key| {
            std::env::var(key).ok()
        })
    }

    pub fn build_with_env<F>(
        self,
        tool_registry: &ToolRegistry,
        kernel: Arc<Kernel>,
        max_turns: usize,
        env: F,
    ) -> Result<ProviderAgent>
    where
        F: Fn(&str) -> Option<String>,
    {
        match self.provider {
            ProviderKind::OpenAI => {
                let api_key_env = self.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
                let api_key = env(api_key_env)
                    .ok_or_else(|| anyhow::anyhow!("missing API key in env '{api_key_env}'"))?;
                let mut builder = rig::providers::openai::Client::builder().api_key(api_key);
                if let Some(base_url) = &self.base_url {
                    builder = builder.base_url(base_url);
                }
                let client = builder.build().context("failed to build OpenAI client")?;
                let agent_builder = client.agent(&self.model).preamble(&self.system_prompt);
                Ok(ProviderAgent::OpenAI(build_agent_with_tools(
                    agent_builder,
                    tool_registry,
                    kernel,
                    max_turns,
                )))
            }
            ProviderKind::OpenRouter => {
                let api_key_env = self.api_key_env.as_deref().unwrap_or("OPENROUTER_API_KEY");
                let api_key = env(api_key_env)
                    .ok_or_else(|| anyhow::anyhow!("missing API key in env '{api_key_env}'"))?;
                let client = rig::providers::openrouter::Client::new(api_key)
                    .context("failed to build OpenRouter client")?;
                let agent_builder = client.agent(&self.model).preamble(&self.system_prompt);
                Ok(ProviderAgent::OpenRouter(build_agent_with_tools(
                    agent_builder,
                    tool_registry,
                    kernel,
                    max_turns,
                )))
            }
            ProviderKind::Gemini => {
                let api_key_env = self.api_key_env.as_deref().unwrap_or("GEMINI_API_KEY");
                let api_key = env(api_key_env)
                    .ok_or_else(|| anyhow::anyhow!("missing API key in env '{api_key_env}'"))?;
                let client = rig::providers::gemini::Client::builder()
                    .api_key(api_key)
                    .build()
                    .context("failed to build Gemini client")?;
                let agent_builder = client.agent(&self.model).preamble(&self.system_prompt);
                Ok(ProviderAgent::Gemini(build_agent_with_tools(
                    agent_builder,
                    tool_registry,
                    kernel,
                    max_turns,
                )))
            }
        }
    }

    pub fn build_without_tools(self) -> Result<ProviderAgent> {
        self.build_without_tools_with_env(|key| std::env::var(key).ok())
    }

    pub fn build_without_tools_with_env<F>(self, env: F) -> Result<ProviderAgent>
    where
        F: Fn(&str) -> Option<String>,
    {
        match self.provider {
            ProviderKind::OpenAI => {
                let api_key_env = self.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
                let api_key = env(api_key_env)
                    .ok_or_else(|| anyhow::anyhow!("missing API key in env '{api_key_env}'"))?;
                let mut builder = rig::providers::openai::Client::builder().api_key(api_key);
                if let Some(base_url) = &self.base_url {
                    builder = builder.base_url(base_url);
                }
                let client = builder.build().context("failed to build OpenAI client")?;
                let agent = client.agent(&self.model).preamble(&self.system_prompt).build();
                Ok(ProviderAgent::OpenAI(agent))
            }
            ProviderKind::OpenRouter => {
                let api_key_env = self.api_key_env.as_deref().unwrap_or("OPENROUTER_API_KEY");
                let api_key = env(api_key_env)
                    .ok_or_else(|| anyhow::anyhow!("missing API key in env '{api_key_env}'"))?;
                let client = rig::providers::openrouter::Client::new(api_key)
                    .context("failed to build OpenRouter client")?;
                let agent = client.agent(&self.model).preamble(&self.system_prompt).build();
                Ok(ProviderAgent::OpenRouter(agent))
            }
            ProviderKind::Gemini => {
                let api_key_env = self.api_key_env.as_deref().unwrap_or("GEMINI_API_KEY");
                let api_key = env(api_key_env)
                    .ok_or_else(|| anyhow::anyhow!("missing API key in env '{api_key_env}'"))?;
                let client = rig::providers::gemini::Client::builder()
                    .api_key(api_key)
                    .build()
                    .context("failed to build Gemini client")?;
                let agent = client.agent(&self.model).preamble(&self.system_prompt).build();
                Ok(ProviderAgent::Gemini(agent))
            }
        }
    }
}

#[derive(Clone)]
pub enum ProviderAgent {
    OpenAI(Agent<openai::responses_api::ResponsesCompletionModel>),
    OpenRouter(Agent<openrouter::CompletionModel>),
    Gemini(Agent<gemini::completion::CompletionModel>),
}

impl ProviderAgent {
    #[allow(dead_code)]
    pub async fn prompt(&self, prompt: impl Into<String>) -> anyhow::Result<String> {
        let prompt = prompt.into();
        match self {
            ProviderAgent::OpenAI(agent) => Ok(agent.prompt(&prompt).await?),
            ProviderAgent::OpenRouter(agent) => Ok(agent.prompt(&prompt).await?),
            ProviderAgent::Gemini(agent) => Ok(agent.prompt(&prompt).await?),
        }
    }

    async fn prompt_with_turns_once(
        &self,
        prompt: &str,
        max_turns: usize,
    ) -> anyhow::Result<String> {
        match self {
            ProviderAgent::OpenAI(agent) => Ok(agent.prompt(prompt).max_turns(max_turns).await?),
            ProviderAgent::OpenRouter(agent) => {
                Ok(agent.prompt(prompt).max_turns(max_turns).await?)
            }
            ProviderAgent::Gemini(agent) => Ok(agent.prompt(prompt).max_turns(max_turns).await?),
        }
    }

    pub async fn prompt_with_turns_retry(
        &self,
        prompt: impl Into<String>,
        max_turns: usize,
        max_retries: usize,
    ) -> Result<String, ProviderError> {
        let prompt = prompt.into();
        let mut attempt = 0;
        loop {
            match self.prompt_with_turns_once(&prompt, max_turns).await {
                Ok(response) => return Ok(response),
                Err(err) => {
                    let mapped = ProviderError::from_anyhow(err.into());
                    if attempt >= max_retries || !mapped.is_retryable() {
                        return Err(mapped);
                    }
                    let backoff = mapped
                        .retry_after()
                        .unwrap_or_else(|| backoff_delay(attempt));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_retries,
                        error = %mapped,
                        "provider call failed, retrying"
                    );
                    sleep(backoff).await;
                    attempt += 1;
                }
            }
        }
    }

    pub async fn prompt_message_with_retry(
        &self,
        message: rig::completion::message::Message,
        max_retries: usize,
    ) -> Result<String, ProviderError> {
        let mut attempt = 0;
        loop {
            let response = match self {
                ProviderAgent::OpenAI(agent) => agent.prompt(message.clone()).await,
                ProviderAgent::OpenRouter(agent) => agent.prompt(message.clone()).await,
                ProviderAgent::Gemini(agent) => agent.prompt(message.clone()).await,
            };
            match response {
                Ok(output) => return Ok(output),
                Err(err) => {
                    let mapped = ProviderError::from_anyhow(err.into());
                    if attempt >= max_retries || !mapped.is_retryable() {
                        return Err(mapped);
                    }
                    let backoff = mapped
                        .retry_after()
                        .unwrap_or_else(|| backoff_delay(attempt));
                    tracing::warn!(
                        attempt = attempt + 1,
                        max_retries,
                        error = %mapped,
                        "provider call failed, retrying"
                    );
                    sleep(backoff).await;
                    attempt += 1;
                }
            }
        }
    }
}

fn backoff_delay(attempt: usize) -> Duration {
    let base_ms = 200u64;
    let shift = attempt.min(8) as u32;
    let multiplier = 1u64.saturating_mul(1u64 << shift);
    let delay_ms = base_ms.saturating_mul(multiplier).min(2000);
    Duration::from_millis(delay_ms)
}
