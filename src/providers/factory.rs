use std::env;
use std::sync::Arc;

use anyhow::{Context, Result};
use rig::agent::Agent;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::providers::{gemini, openai, openrouter};
use rig::tool::ToolDyn;

use crate::config::Config;
use crate::kernel::core::Kernel;
use crate::tools::registry::ToolRegistry;
use crate::tools::rig_wrapper::KernelBackedTool;

#[derive(Debug, Clone, Copy)]
pub enum ProviderKind {
    OpenAI,
    OpenRouter,
    Gemini,
}

impl ProviderKind {
    pub fn from_str(value: &str) -> Result<Self> {
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
        let builder = Self::build_agent_builder(config)?;
        Ok(builder.build(tool_registry, kernel, config.max_turns()))
    }

    pub fn build_agent_builder(config: &Config) -> Result<ProviderAgentBuilder> {
        ProviderAgentBuilder::new(config)
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
            provider: ProviderKind::from_str(config.provider())?,
            model: config.model().to_string(),
            system_prompt: config.system_prompt().to_string(),
            base_url: config.base_url.clone(),
            api_key_env: config.api_key_env.clone(),
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

impl ProviderAgentBuilder {
    pub fn build(
        self,
        tool_registry: &ToolRegistry,
        kernel: Arc<Kernel>,
        max_turns: usize,
    ) -> ProviderAgent {
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
    ) -> ProviderAgent
    where
        F: Fn(&str) -> Option<String>,
    {
        match self.provider {
            ProviderKind::OpenAI => {
                let api_key_env = self.api_key_env.as_deref().unwrap_or("OPENAI_API_KEY");
                let api_key = env(api_key_env).unwrap_or_default();
                let mut builder = rig::providers::openai::Client::builder().api_key(api_key);
                if let Some(base_url) = &self.base_url {
                    builder = builder.base_url(base_url);
                }
                let client = builder.build().expect("openai client");
                let agent_builder = client.agent(&self.model).preamble(&self.system_prompt);
                ProviderAgent::OpenAI(build_agent_with_tools(
                    agent_builder,
                    tool_registry,
                    kernel,
                    max_turns,
                ))
            }
            ProviderKind::OpenRouter => {
                let api_key_env = self.api_key_env.as_deref().unwrap_or("OPENROUTER_API_KEY");
                let api_key = env(api_key_env).unwrap_or_default();
                let client =
                    rig::providers::openrouter::Client::new(api_key).expect("openrouter client");
                let agent_builder = client.agent(&self.model).preamble(&self.system_prompt);
                ProviderAgent::OpenRouter(build_agent_with_tools(
                    agent_builder,
                    tool_registry,
                    kernel,
                    max_turns,
                ))
            }
            ProviderKind::Gemini => {
                let api_key_env = self.api_key_env.as_deref().unwrap_or("GEMINI_API_KEY");
                let api_key = env(api_key_env).unwrap_or_default();
                let client = rig::providers::gemini::Client::builder()
                    .api_key(api_key)
                    .build()
                    .expect("gemini client");
                let agent_builder = client.agent(&self.model).preamble(&self.system_prompt);
                ProviderAgent::Gemini(build_agent_with_tools(
                    agent_builder,
                    tool_registry,
                    kernel,
                    max_turns,
                ))
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

    pub async fn prompt_with_turns(
        &self,
        prompt: impl Into<String>,
        max_turns: usize,
    ) -> anyhow::Result<String> {
        let prompt = prompt.into();
        match self {
            ProviderAgent::OpenAI(agent) => Ok(agent.prompt(&prompt).max_turns(max_turns).await?),
            ProviderAgent::OpenRouter(agent) => {
                Ok(agent.prompt(&prompt).max_turns(max_turns).await?)
            }
            ProviderAgent::Gemini(agent) => Ok(agent.prompt(&prompt).max_turns(max_turns).await?),
        }
    }
}
