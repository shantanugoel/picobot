use std::sync::Arc;

use picobot::config::SchedulerConfig;
use picobot::kernel::core::Kernel;
use picobot::providers::factory::{ProviderAgentBuilder, ProviderKind};
use picobot::scheduler::executor::JobExecutor;
use picobot::scheduler::service::SchedulerService;
use picobot::scheduler::store::ScheduleStore;
use picobot::session::db::SqliteStore;
use picobot::tools::registry::ToolRegistry;

#[test]
fn scheduler_list_by_session_filters_results() {
    let dir = std::env::temp_dir().join(format!("picobot-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = SqliteStore::new(dir.join("picobot.db").to_string_lossy().to_string());
    store.touch().unwrap();
    let schedule_store = ScheduleStore::new(store.clone());
    let registry = Arc::new(ToolRegistry::new());
    let kernel = Kernel::new(Arc::clone(&registry));
    let agent_builder = ProviderAgentBuilder::from_parts(
        ProviderKind::OpenAI,
        "gpt-4o-mini".to_string(),
        "test".to_string(),
        None,
        None,
    );
    let mut scheduler_config = SchedulerConfig::default();
    scheduler_config.enabled = Some(true);
    let executor = JobExecutor::new(
        Arc::new(kernel),
        schedule_store.clone(),
        scheduler_config.clone(),
        agent_builder,
        None,
        picobot::config::Config::default(),
    );
    let scheduler = SchedulerService::new(schedule_store.clone(), executor, scheduler_config);

    let mut capabilities = picobot::kernel::permissions::CapabilitySet::empty();
    capabilities.insert(picobot::kernel::permissions::Permission::Schedule {
        action: "create".to_string(),
    });
    let request = picobot::scheduler::job::CreateJobRequest {
        name: "job".to_string(),
        schedule_type: picobot::scheduler::job::ScheduleType::Interval,
        schedule_expr: "60".to_string(),
        task_prompt: "ping".to_string(),
        session_id: Some("session-1".to_string()),
        user_id: "user".to_string(),
        channel_id: None,
        capabilities,
        creator: picobot::scheduler::job::Principal {
            principal_type: picobot::scheduler::job::PrincipalType::User,
            id: "user".to_string(),
        },
        enabled: true,
        max_executions: None,
        created_by_system: false,
        metadata: None,
    };
    scheduler.create_job(request).expect("create job");

    let jobs = scheduler
        .store()
        .list_jobs_by_user_with_session("user", "session-1")
        .expect("list jobs");
    assert_eq!(jobs.len(), 1);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn scheduler_cancel_disables_persisted_job() {
    let dir = std::env::temp_dir().join(format!("picobot-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = SqliteStore::new(dir.join("picobot.db").to_string_lossy().to_string());
    store.touch().unwrap();
    let schedule_store = ScheduleStore::new(store.clone());
    let registry = Arc::new(ToolRegistry::new());
    let kernel = Kernel::new(Arc::clone(&registry));
    let agent_builder = ProviderAgentBuilder::from_parts(
        ProviderKind::OpenAI,
        "gpt-4o-mini".to_string(),
        "test".to_string(),
        None,
        None,
    );
    let mut scheduler_config = SchedulerConfig::default();
    scheduler_config.enabled = Some(true);
    let executor = JobExecutor::new(
        Arc::new(kernel),
        schedule_store.clone(),
        scheduler_config.clone(),
        agent_builder,
        None,
        picobot::config::Config::default(),
    );
    let scheduler = SchedulerService::new(schedule_store.clone(), executor, scheduler_config);

    let mut capabilities = picobot::kernel::permissions::CapabilitySet::empty();
    capabilities.insert(picobot::kernel::permissions::Permission::Schedule {
        action: "create".to_string(),
    });
    let request = picobot::scheduler::job::CreateJobRequest {
        name: "job".to_string(),
        schedule_type: picobot::scheduler::job::ScheduleType::Interval,
        schedule_expr: "60".to_string(),
        task_prompt: "ping".to_string(),
        session_id: Some("session-1".to_string()),
        user_id: "user".to_string(),
        channel_id: None,
        capabilities,
        creator: picobot::scheduler::job::Principal {
            principal_type: picobot::scheduler::job::PrincipalType::User,
            id: "user".to_string(),
        },
        enabled: true,
        max_executions: None,
        created_by_system: false,
        metadata: None,
    };
    let job = scheduler.create_job(request).expect("create job");

    scheduler
        .cancel_job_and_disable(&job.id)
        .expect("cancel job");

    let stored = scheduler
        .store()
        .get_job(&job.id)
        .expect("get job")
        .expect("job exists");
    assert!(!stored.enabled);

    std::fs::remove_dir_all(&dir).ok();
}
