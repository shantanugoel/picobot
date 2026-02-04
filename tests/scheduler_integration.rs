use std::sync::Arc;

use picobot::config::{Config, ModelConfig, RoutingConfig, SchedulerConfig};
use picobot::kernel::agent::Kernel;
use picobot::models::router::ModelRegistry;
use picobot::scheduler::executor::JobExecutor;
use picobot::scheduler::job::{
    CreateJobRequest, ExecutionStatus, Principal, PrincipalType, ScheduleType,
};
use picobot::scheduler::service::SchedulerService;
use picobot::scheduler::store::ScheduleStore;
use picobot::session::db::SqliteStore;
use picobot::tools::registry::ToolRegistry;

fn temp_store() -> (ScheduleStore, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("picobot-scheduler-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("schedules.db");
    let store = ScheduleStore::new(SqliteStore::new(path.to_string_lossy().to_string()));
    let _ = store.store().touch();
    (store, dir)
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
    };
    ModelRegistry::from_config(&config).unwrap()
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
    let models = Arc::new(test_models());
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
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let updated = store.get_job(&job.id).unwrap().unwrap();
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
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let updated = store.get_job(&job.id).unwrap().unwrap();
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
    service.tick().await;
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let executions = store.list_executions_for_job(&job.id, 10, 0).unwrap();
    assert_eq!(executions.len(), 1);
    assert_eq!(executions[0].status, ExecutionStatus::Completed);
    std::fs::remove_dir_all(dir).ok();
}
