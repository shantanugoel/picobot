use std::sync::Arc;

use picobot::config::SchedulerConfig;
use picobot::kernel::agent::Kernel;
use picobot::models::router::ModelRegistry;
use picobot::models::traits::{Model, ModelError};
use picobot::models::types::{ModelEvent, ModelInfo, ModelRequest, ModelResponse};
use picobot::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};
use picobot::scheduler::executor::JobExecutor;
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

#[derive(Debug)]
struct StaticModel;

#[async_trait::async_trait]
impl Model for StaticModel {
    fn info(&self) -> ModelInfo {
        ModelInfo {
            id: "static".to_string(),
            provider: "test".to_string(),
            model: "static".to_string(),
        }
    }

    async fn complete(&self, _req: ModelRequest) -> Result<ModelResponse, ModelError> {
        Ok(ModelResponse::Text("ok".to_string()))
    }

    async fn stream(&self, _req: ModelRequest) -> Result<Vec<ModelEvent>, ModelError> {
        Ok(vec![ModelEvent::Done(ModelResponse::Text("ok".to_string()))])
    }
}

fn build_service(store: ScheduleStore) -> SchedulerService {
    let registry = ToolRegistry::new();
    let mut kernel = Kernel::new(registry, std::path::PathBuf::from("/"));
    let mut capabilities = picobot::kernel::permissions::CapabilitySet::empty();
    capabilities.insert(picobot::kernel::permissions::Permission::Schedule {
        action: "create".to_string(),
    });
    kernel.set_capabilities(capabilities);
    let kernel = Arc::new(kernel);
    let models = Arc::new(
        ModelRegistry::from_models("static", vec![Arc::new(StaticModel)]).unwrap(),
    );
    let config = SchedulerConfig {
        enabled: Some(true),
        tick_interval_secs: Some(1),
        ..Default::default()
    };
    let executor = JobExecutor::new(kernel, models, store.clone(), config.clone());
    SchedulerService::new(store, executor, config)
}

#[tokio::test]
async fn interval_job_reschedules_correctly() {
    let (store, dir) = temp_store();
    let service = build_service(store.clone());
    let now = chrono::Utc::now();
    let request = CreateJobRequest {
        name: "interval".to_string(),
        schedule_type: ScheduleType::Interval,
        schedule_expr: "2".to_string(),
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
        max_executions: Some(2),
        metadata: None,
    };
    let job = store.create_job(request, now).unwrap();
    service.tick().await;
    let updated = wait_for_job_update(&store, &job.id, Duration::from_secs(2)).await;
    assert!(updated.next_run_at > now);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn once_job_disables_after_execution() {
    let (store, dir) = temp_store();
    let service = build_service(store.clone());
    let now = chrono::Utc::now();
    let request = CreateJobRequest {
        name: "once".to_string(),
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
        metadata: None,
    };
    let job = store.create_job(request, now).unwrap();
    service.tick().await;
    let updated = wait_for_job_update(&store, &job.id, Duration::from_secs(2)).await;
    assert!(!updated.enabled);
    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn execution_recorded_in_history() {
    let (store, dir) = temp_store();
    let service = build_service(store.clone());
    let now = chrono::Utc::now();
    let request = CreateJobRequest {
        name: "history".to_string(),
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
        metadata: None,
    };
    let job = store.create_job(request, now).unwrap();
    let job_id = job.id.clone();
    service.tick().await;
    let updated = wait_for_job_update(&store, &job_id, Duration::from_secs(4)).await;
    assert_eq!(updated.execution_count, 1);
    assert!(updated.last_run_at.is_some());
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
            if job.last_run_at.is_some() || !job.enabled {
                return job;
            }
        }
        if start.elapsed() >= timeout {
            panic!("job update timed out");
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

// execution history is reflected on the schedule record

// execution history is updated synchronously after completion
