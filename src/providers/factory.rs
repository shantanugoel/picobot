use std::env;
use std::sync::Arc;

use anyhow::{Context, Result};
use rig::agent::Agent;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::providers::{gemini, openai, openrouter};
use rig::tool::ToolDyn;

use crate::config::Config;
use crate::kernel::kernel::Kernel;
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

    pub fn build_openrouter_client(config: &Config) -> Result<openrouter::Client> {
        let api_key_env = config.api_key_env.as_deref().unwrap_or("OPENROUTER_API_KEY");
        let api_key = env::var(api_key_env)
            .with_context(|| format!("missing API key in env '{api_key_env}'"))?;
        Ok(openrouter::Client::new(api_key)?)
    }

    pub fn build_gemini_client(config: &Config) -> Result<gemini::Client> {
        let api_key_env = config.api_key_env.as_deref().unwrap_or("GEMINI_API_KEY");
        let api_key = env::var(api_key_env)
            .with_context(|| format!("missing API key in env '{api_key_env}'"))?;
        Ok(gemini::Client::builder().api_key(api_key).build()?)
    }

    pub fn build_agent(
        config: &Config,
        tool_registry: &ToolRegistry,
        kernel: Arc<Kernel>,
    ) -> Result<ProviderAgent> {
        let provider = ProviderKind::from_str(config.provider())?;

        match provider {
            ProviderKind::OpenAI => {
                let client = Self::build_openai_client(config)?;
                let agent = build_agent_with_tools(
                    client.agent(config.model()).preamble(config.system_prompt()),
                    tool_registry,
                    kernel,
                    config.max_turns(),
                );
                Ok(ProviderAgent::OpenAI(agent))
            }
            ProviderKind::OpenRouter => {
                let client = Self::build_openrouter_client(config)?;
                let agent = build_agent_with_tools(
                    client.agent(config.model()).preamble(config.system_prompt()),
                    tool_registry,
                    kernel,
                    config.max_turns(),
                );
                Ok(ProviderAgent::OpenRouter(agent))
            }
            ProviderKind::Gemini => {
                let client = Self::build_gemini_client(config)?;
                let agent = build_agent_with_tools(
                    client.agent(config.model()).preamble(config.system_prompt()),
                    tool_registry,
                    kernel,
                    config.max_turns(),
                );
                Ok(ProviderAgent::Gemini(agent))
            }
        }
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
pub enum ProviderAgent {
    OpenAI(Agent<openai::responses_api::ResponsesCompletionModel>),
    OpenRouter(Agent<openrouter::CompletionModel>),
    Gemini(Agent<gemini::completion::CompletionModel>),
}

impl ProviderAgent {
    pub async fn prompt(&self, prompt: impl Into<String>) -> anyhow::Result<String> {
        let prompt = prompt.into();
        match self {
            ProviderAgent::OpenAI(agent) => Ok(agent.prompt(&prompt).await?),
            ProviderAgent::OpenRouter(agent) => Ok(agent.prompt(&prompt).await?),
            ProviderAgent::Gemini(agent) => Ok(agent.prompt(&prompt).await?),
        }
    }
}
