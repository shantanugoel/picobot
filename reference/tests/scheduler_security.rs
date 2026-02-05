use std::sync::Arc;

use async_trait::async_trait;
use picobot::config::SchedulerConfig;
use picobot::kernel::agent::Kernel;
use picobot::kernel::permissions::{CapabilitySet, Permission};
use picobot::scheduler::executor::JobExecutor;
use picobot::scheduler::job::{CreateJobRequest, Principal, PrincipalType, ScheduleType};
use picobot::scheduler::service::SchedulerService;
use picobot::scheduler::store::ScheduleStore;
use picobot::session::db::SqliteStore;
use picobot::tools::registry::ToolRegistry;
use picobot::tools::schedule::ScheduleTool;
use picobot::tools::traits::{Tool, ToolError, ToolOutput};

fn temp_store() -> (ScheduleStore, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("picobot-scheduler-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("schedules.db");
    let store = ScheduleStore::new(SqliteStore::new(path.to_string_lossy().to_string()));
    let _ = store.store().touch();
    (store, dir)
}

fn schedule_service(kernel: Arc<Kernel>) -> (Arc<SchedulerService>, std::path::PathBuf) {
    let (store, dir) = temp_store();
    let config = SchedulerConfig {
        enabled: Some(true),
        ..Default::default()
    };
    let executor = JobExecutor::new(
        Arc::clone(&kernel),
        Arc::new(test_models()),
        store.clone(),
        config.clone(),
    );
    let service = Arc::new(SchedulerService::new(store, executor, config));
    (service, dir)
}

fn test_models() -> picobot::models::router::ModelRegistry {
    use picobot::config::{Config, ModelConfig, RoutingConfig};

    let config = Config {
        agent: None,
        models: vec![ModelConfig {
            id: "static".to_string(),
            provider: "openai".to_string(),
            model: "stub".to_string(),
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
    picobot::models::router::ModelRegistry::from_config(&config).unwrap()
}

#[derive(Debug)]
struct NeedsShell;

#[async_trait]
impl Tool for NeedsShell {
    fn name(&self) -> &'static str {
        "needs_shell"
    }

    fn description(&self) -> &'static str {
        "requires shell permission"
    }

    fn schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }

    fn required_permissions(
        &self,
        _ctx: &picobot::kernel::context::ToolContext,
        _input: &serde_json::Value,
    ) -> Result<Vec<Permission>, ToolError> {
        Ok(vec![Permission::ShellExec {
            allowed_commands: Some(vec!["bash".to_string()]),
        }])
    }

    async fn execute(
        &self,
        _ctx: &picobot::kernel::context::ToolContext,
        _input: serde_json::Value,
    ) -> Result<ToolOutput, ToolError> {
        Ok(serde_json::json!({"ok": true}))
    }
}

#[tokio::test]
async fn job_cannot_exceed_snapshot_capabilities() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(NeedsShell)).unwrap();
    let kernel = Kernel::new(registry, std::path::PathBuf::from("/"));

    let job = CreateJobRequest {
        name: "limited".to_string(),
        schedule_type: ScheduleType::Once,
        schedule_expr: "now".to_string(),
        task_prompt: "ping".to_string(),
        session_id: None,
        user_id: "user".to_string(),
        channel_id: None,
        capabilities: CapabilitySet::empty(),
        creator: Principal {
            principal_type: PrincipalType::User,
            id: "user".to_string(),
        },
        enabled: true,
        max_executions: Some(1),
        created_by_system: false,
        metadata: None,
    };
    let mut scoped = kernel.clone_with_context(Some(job.user_id.clone()), None);
    scoped.set_capabilities(job.capabilities.clone());
    let tool = scoped.tool_registry().get("needs_shell").unwrap();
    let result = scoped.invoke_tool(tool, serde_json::json!({})).await;
    assert!(matches!(result, Err(ToolError::PermissionDenied { .. })));
}

#[test]
fn schedule_creation_requires_permission() {
    let capabilities = CapabilitySet::empty();
    let request = CreateJobRequest {
        name: "no-permission".to_string(),
        schedule_type: ScheduleType::Once,
        schedule_expr: "now".to_string(),
        task_prompt: "ping".to_string(),
        session_id: None,
        user_id: "user".to_string(),
        channel_id: None,
        capabilities,
        creator: Principal {
            principal_type: PrincipalType::User,
            id: "user".to_string(),
        },
        enabled: true,
        max_executions: None,
        created_by_system: false,
        metadata: None,
    };

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ScheduleTool)).unwrap();
    let kernel = Arc::new(Kernel::new(registry, std::path::PathBuf::from("/")));
    let (service, dir) = schedule_service(kernel);

    let result = service.create_job(request);
    assert!(result.is_err());

    std::fs::remove_dir_all(dir).ok();
}

#[tokio::test]
async fn malicious_prompt_does_not_escalate_permissions() {
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(ScheduleTool)).unwrap();
    let mut kernel = Kernel::new(registry, std::path::PathBuf::from("/"));
    let mut capabilities = CapabilitySet::empty();
    capabilities.insert(Permission::Schedule {
        action: "create".to_string(),
    });
    kernel.set_capabilities(capabilities);

    let kernel = Arc::new(kernel);
    let (service, dir) = schedule_service(Arc::clone(&kernel));

    let mut scoped = kernel.clone_with_context(Some("user".to_string()), None);
    scoped.set_scheduler(Some(Arc::clone(&service)));

    let tool = scoped.tool_registry().get("schedule").unwrap();
    let result = scoped
        .invoke_tool(
            tool,
            serde_json::json!({
                "action": "create",
                "name": "escalate",
                "schedule_type": "once",
                "schedule_expr": "now",
                "task_prompt": "do stuff",
                "capabilities": ["shell:*"]
            }),
        )
        .await;
    assert!(matches!(result, Ok(_)));

    std::fs::remove_dir_all(dir).ok();
}
