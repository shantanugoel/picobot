use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{Config, ModelConfig};
use crate::models::genai_adapter::{GenaiModel, build_client};
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
    models: HashMap<ModelId, Arc<dyn Model>>,
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

        let mut models: HashMap<ModelId, Arc<dyn Model>> = HashMap::new();
        for model_config in &config.models {
            let model = build_model(model_config)?;
            models.insert(model.info().id.clone(), Arc::new(model));
        }

        if !models.contains_key(&default_id) {
            return Err(ModelRegistryError::UnknownDefault(default_id));
        }

        Ok(Self { models, default_id })
    }

    pub fn from_models(
        default_id: &str,
        models: Vec<Arc<dyn Model>>,
    ) -> Result<Self, ModelRegistryError> {
        if models.is_empty() {
            return Err(ModelRegistryError::EmptyConfig);
        }
        let mut map = HashMap::new();
        for model in models {
            map.insert(model.info().id.clone(), model);
        }
        if !map.contains_key(default_id) {
            return Err(ModelRegistryError::UnknownDefault(default_id.to_string()));
        }
        Ok(Self {
            models: map,
            default_id: default_id.to_string(),
        })
    }

    pub fn default_model(&self) -> &dyn Model {
        self.models
            .get(&self.default_id)
            .map(|model| model.as_ref())
            .expect("default model missing")
    }

    pub fn default_model_arc(&self) -> Arc<dyn Model> {
        self.models
            .get(&self.default_id)
            .cloned()
            .expect("default model missing")
    }

    pub fn default_id(&self) -> &str {
        &self.default_id
    }

    pub fn get(&self, id: &str) -> Option<&dyn Model> {
        self.models.get(id).map(|model| model.as_ref())
    }

    pub fn get_arc(&self, id: &str) -> Option<Arc<dyn Model>> {
        self.models.get(id).cloned()
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

fn build_model(config: &ModelConfig) -> Result<GenaiModel, ModelRegistryError> {
    let provider = config.provider.to_lowercase();
    let info = ModelInfo {
        id: config.id.clone(),
        provider: config.provider.clone(),
        model: config.model.clone(),
    };
    let (client, adapter_kind) = build_client(
        &provider,
        &config.model,
        config.api_key_env.as_deref(),
        config.base_url.as_deref(),
    )
    .map_err(|err: crate::models::traits::ModelError| {
        ModelRegistryError::InitializationFailed(config.id.clone(), err.to_string())
    })?;
    Ok(GenaiModel::new(info, client, adapter_kind))
}

#[cfg(test)]
mod tests {
    use super::{ModelRegistry, ModelRegistryError};
    use crate::config::{Config, ModelConfig, RoutingConfig};

    #[test]
    fn registry_requires_default_model() {
        let config = Config {
            models: vec![ModelConfig {
                id: "default".to_string(),
                provider: "openai".to_string(),
                model: "gpt-4o".to_string(),
                api_key_env: None,
                base_url: None,
            }],
            routing: Some(RoutingConfig { default: None }),
            agent: None,
            permissions: None,
            logging: None,
            server: None,
            channels: None,
            session: None,
            data: None,
            scheduler: None,
            notifications: None,
            heartbeats: None,
        };

        let result = ModelRegistry::from_config(&config);
        assert!(matches!(result, Err(ModelRegistryError::MissingDefault)));
    }
}
