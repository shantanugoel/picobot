use std::sync::Arc;

use picobot::config::{Config, ModelConfig, RoutingConfig, SchedulerConfig};
use picobot::kernel::agent::Kernel;
use picobot::models::router::ModelRegistry;
use picobot::models::traits::{Model, ModelError};
use picobot::models::types::{ModelEvent, ModelInfo, ModelRequest, ModelResponse};
use picobot::scheduler::executor::JobExecutor;
use picobot::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};
use picobot::scheduler::service::SchedulerService;
use picobot::scheduler::store::ScheduleStore;
use picobot::session::db::SqliteStore;
use picobot::tools::registry::ToolRegistry;
use std::time::Duration;

fn temp_store() -> (ScheduleStore, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("picobot-scheduler-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("schedules.db");
    let store = ScheduleStore::new(SqliteStore::new(path.to_string_lossy().to_string()));
    let _ = store.store().touch();
    (store, dir)
}

fn scheduler_service() -> (Arc<SchedulerService>, std::path::PathBuf) {
    let (store, dir) = temp_store();
    let registry = ToolRegistry::new();
    let mut kernel = Kernel::new(registry, std::path::PathBuf::from("/"));
    let mut capabilities = picobot::kernel::permissions::CapabilitySet::empty();
    capabilities.insert(picobot::kernel::permissions::Permission::Schedule {
        action: "create".to_string(),
    });
    kernel.set_capabilities(capabilities);
    let kernel = Arc::new(kernel);
    let models = Arc::new(test_models());
    let config = SchedulerConfig {
        enabled: Some(true),
        tick_interval_secs: Some(1),
        job_timeout_secs: Some(1),
        ..Default::default()
    };
    let executor = JobExecutor::new(Arc::clone(&kernel), models, store.clone(), config.clone());
    let service = Arc::new(SchedulerService::new(store, executor, config));
    (service, dir)
}

fn test_models() -> ModelRegistry {
    let config = Config {
        agent: None,
        models: vec![ModelConfig {
            id: "static".to_string(),
            provider: "openai".to_string(),
            model: "gpt-4o".to_string(),
            api_key_env: None,
            base_url: None,
        }],
        routing: Some(RoutingConfig {
            default: Some("static".to_string()),
        }),
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
    ModelRegistry::from_config(&config).unwrap()
}

#[derive(Debug)]
struct SlowModel;

#[async_trait::async_trait]
impl Model for SlowModel {
    fn info(&self) -> ModelInfo {
        ModelInfo {
            id: "slow".to_string(),
            provider: "test".to_string(),
            model: "slow".to_string(),
        }
    }

    async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, ModelError> {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        Ok(ModelResponse::Text("ok".to_string()))
    }

    async fn stream(&self, _req: ModelRequest) -> Result<Vec<ModelEvent>, ModelError> {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        Ok(vec![ModelEvent::Done(ModelResponse::Text(
            "ok".to_string(),
        ))])
    }
}

#[tokio::test]
async fn cancel_running_job() {
    let (store, dir) = temp_store();
    let registry = ToolRegistry::new();
    let mut kernel = Kernel::new(registry, std::path::PathBuf::from("/"));
    let mut capabilities = picobot::kernel::permissions::CapabilitySet::empty();
    capabilities.insert(picobot::kernel::permissions::Permission::Schedule {
        action: "create".to_string(),
    });
    kernel.set_capabilities(capabilities);
    let kernel = Arc::new(kernel);
    let models = Arc::new(ModelRegistry::from_models("slow", vec![Arc::new(SlowModel)]).unwrap());
    let config = SchedulerConfig {
        enabled: Some(true),
        job_timeout_secs: Some(5),
        ..Default::default()
    };
    let executor = JobExecutor::new(Arc::clone(&kernel), models, store.clone(), config);
    let now = chrono::Utc::now();
    let request = CreateJobRequest {
        name: "cancel-me".to_string(),
        schedule_type: ScheduleType::Once,
        schedule_expr: "now".to_string(),
        task_prompt: "ping".to_string(),
        session_id: None,
        user_id: "user".to_string(),
        channel_id: None,
        capabilities: picobot::kernel::permissions::CapabilitySet::empty(),
        creator: Principal {
            principal_type: PrincipalType::User,
            id: "user".to_string(),
        },
        enabled: true,
        max_executions: None,
        created_by_system: false,
        metadata: None,
    };
    let job = store.create_job(request, now).unwrap();
    let job_id = job.id.clone();
    let executor_clone = executor.clone();
    let handle = tokio::spawn(async move {
        executor_clone.execute(job).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert!(executor.cancel_job(&job_id));
    handle.await.unwrap();

    let updated = store.get_job(&job_id).unwrap().unwrap();
    assert_eq!(updated.execution_count, 0);

    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn timeout_marks_job_as_timeout() {
    let (store, dir) = temp_store();
    let registry = ToolRegistry::new();
    let mut kernel = Kernel::new(registry, std::path::PathBuf::from("/"));
    let mut capabilities = picobot::kernel::permissions::CapabilitySet::empty();
    capabilities.insert(picobot::kernel::permissions::Permission::Schedule {
        action: "create".to_string(),
    });
    kernel.set_capabilities(capabilities);
    let kernel = Arc::new(kernel);
    let models = Arc::new(test_models());
    let config = SchedulerConfig {
        enabled: Some(true),
        job_timeout_secs: Some(0),
        ..Default::default()
    };
    let executor = JobExecutor::new(Arc::clone(&kernel), models, store.clone(), config);
    let now = chrono::Utc::now();
    let request = CreateJobRequest {
        name: "timeout".to_string(),
        schedule_type: ScheduleType::Once,
        schedule_expr: "now".to_string(),
        task_prompt: "ping".to_string(),
        session_id: None,
        user_id: "user".to_string(),
        channel_id: None,
        capabilities: picobot::kernel::permissions::CapabilitySet::empty(),
        creator: Principal {
            principal_type: PrincipalType::User,
            id: "user".to_string(),
        },
        enabled: true,
        max_executions: None,
        created_by_system: false,
        metadata: None,
    };
    let job = store.create_job(request, now).unwrap();
    let job_id = job.id.clone();
    executor.execute(job).await;
    let updated = wait_for_job_update(&store, &job_id, Duration::from_secs(2)).await;
    assert_eq!(updated.execution_count, 0);
    assert!(updated.last_error.as_deref() == Some("job timed out"));
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn delete_cancels_then_removes_schedule() {
    let (service, dir) = scheduler_service();
    let now = chrono::Utc::now();
    let request = CreateJobRequest {
        name: "delete".to_string(),
        schedule_type: ScheduleType::Once,
        schedule_expr: "now".to_string(),
        task_prompt: "ping".to_string(),
        session_id: None,
        user_id: "user".to_string(),
        channel_id: None,
        capabilities: picobot::kernel::permissions::CapabilitySet::empty(),
        creator: Principal {
            principal_type: PrincipalType::User,
            id: "user".to_string(),
        },
        enabled: true,
        max_executions: None,
        created_by_system: false,
        metadata: None,
    };
    let job = service.store().create_job(request, now).unwrap();
    service.delete_job_with_cancel(&job.id).unwrap();
    let fetched = service.store().get_job(&job.id).unwrap();
    assert!(fetched.is_none());
    std::fs::remove_dir_all(dir).ok();
}

async fn wait_for_job_update(
    store: &ScheduleStore,
    job_id: &str,
    timeout: Duration,
) -> picobot::scheduler::job::ScheduledJob {
    let start = std::time::Instant::now();
    loop {
        if let Some(job) = store.get_job(job_id).unwrap() {
            if job.last_error.is_some() || !job.enabled {
                return job;
            }
        }
        if start.elapsed() >= timeout {
            panic!("job update timed out");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}
