use std::collections::HashMap;

use crate::config::{Config, ModelConfig};
use crate::models::openai_compat::OpenAICompatModel;
use crate::models::traits::Model;
use crate::models::types::{ModelId, ModelInfo, ModelRequest};

#[derive(Debug, thiserror::Error)]
pub enum ModelRegistryError {
    #[error("No models configured")]
    EmptyConfig,
    #[error("Missing default model id")]
    MissingDefault,
    #[error("Unknown default model id '{0}'")]
    UnknownDefault(String),
    #[error("Unsupported provider '{0}'")]
    UnsupportedProvider(String),
    #[error("Model '{0}' failed to initialize: {1}")]
    InitializationFailed(String, String),
}

pub struct ModelRegistry {
    models: HashMap<ModelId, Box<dyn Model>>,
    default_id: ModelId,
}

impl ModelRegistry {
    pub fn from_config(config: &Config) -> Result<Self, ModelRegistryError> {
        if config.models.is_empty() {
            return Err(ModelRegistryError::EmptyConfig);
        }

        let default_id = config
            .routing
            .as_ref()
            .and_then(|routing| routing.default.clone())
            .ok_or(ModelRegistryError::MissingDefault)?;

        let mut models: HashMap<ModelId, Box<dyn Model>> = HashMap::new();
        for model_config in &config.models {
            let model = build_model(model_config)?;
            models.insert(model.info().id.clone(), Box::new(model));
        }

        if !models.contains_key(&default_id) {
            return Err(ModelRegistryError::UnknownDefault(default_id));
        }

        Ok(Self { models, default_id })
    }

    pub fn default_model(&self) -> &dyn Model {
        self.models
            .get(&self.default_id)
            .map(|model| model.as_ref())
            .expect("default model missing")
    }

    pub fn default_id(&self) -> &str {
        &self.default_id
    }

    pub fn get(&self, id: &str) -> Option<&dyn Model> {
        self.models.get(id).map(|model| model.as_ref())
    }

    pub fn model_infos(&self) -> Vec<ModelInfo> {
        let mut infos = self
            .models
            .values()
            .map(|model| model.info())
            .collect::<Vec<_>>();
        infos.sort_by(|a, b| a.id.cmp(&b.id));
        infos
    }
}

pub trait ModelRouter: Send + Sync {
    fn select_model(&self, request: &ModelRequest, task_hint: Option<&str>) -> ModelId;
}

#[derive(Debug)]
pub struct SingleModelRouter {
    model_id: ModelId,
}

impl SingleModelRouter {
    pub fn new(model_id: ModelId) -> Self {
        Self { model_id }
    }
}

impl ModelRouter for SingleModelRouter {
    fn select_model(&self, _request: &ModelRequest, _task_hint: Option<&str>) -> ModelId {
        self.model_id.clone()
    }
}

fn build_model(config: &ModelConfig) -> Result<OpenAICompatModel, ModelRegistryError> {
    let provider = config.provider.to_lowercase();
    if provider != "openai" && provider != "ollama" && provider != "openrouter" {
        return Err(ModelRegistryError::UnsupportedProvider(
            config.provider.clone(),
        ));
    }

    let mut openai_config = async_openai::config::OpenAIConfig::new();
    if let Some(base_url) = &config.base_url {
        openai_config = openai_config.with_api_base(base_url);
    }

    if let Some(api_key_env) = &config.api_key_env
        && let Ok(api_key) = std::env::var(api_key_env)
    {
        openai_config = openai_config.with_api_key(api_key);
    }

    let client = async_openai::Client::with_config(openai_config);
    let info = ModelInfo {
        id: config.id.clone(),
        provider: config.provider.clone(),
        model: config.model.clone(),
    };
    Ok(OpenAICompatModel::new(info, client))
}
