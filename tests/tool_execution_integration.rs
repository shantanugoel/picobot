use std::sync::Arc;

use serde_json::json;

use async_trait::async_trait;

use picobot::kernel::core::Kernel;
use picobot::kernel::permissions::{CapabilitySet, PathPattern, Permission};
use picobot::notifications::channel::{NotificationChannel, NotificationRequest};
use picobot::notifications::queue::{NotificationQueue, NotificationQueueConfig};
use picobot::notifications::service::NotificationService;
use picobot::providers::factory::{ProviderAgentBuilder, ProviderKind};
use picobot::scheduler::executor::JobExecutor;
use picobot::scheduler::service::SchedulerService;
use picobot::scheduler::store::ScheduleStore;
use picobot::tools::filesystem::FilesystemTool;
use picobot::tools::notify::NotifyTool;
use picobot::tools::registry::ToolRegistry;
use picobot::tools::schedule::ScheduleTool;

#[tokio::test]
async fn filesystem_read_allowed_via_kernel() {
    let dir = std::env::temp_dir().join(format!("picobot-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("data.txt");
    std::fs::write(&file, "hello").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FilesystemTool::new())).unwrap();
    let registry = Arc::new(registry);

    let canonical_dir = dir.canonicalize().unwrap();
    let mut capabilities = CapabilitySet::empty();
    capabilities.insert(Permission::FileRead {
        path: PathPattern(format!("{}/**", canonical_dir.to_string_lossy())),
    });
    let kernel = Kernel::new(Arc::clone(&registry)).with_capabilities(capabilities);

    let tool = kernel.tool_registry().get("filesystem").unwrap();
    let result = kernel
        .invoke_tool(
            tool.as_ref(),
            json!({"operation": "read", "path": file.to_string_lossy()}),
        )
        .await;
    assert!(result.is_ok());

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn filesystem_read_denied_via_kernel() {
    let dir = std::env::temp_dir().join(format!("picobot-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("data.txt");
    std::fs::write(&file, "hello").unwrap();

    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FilesystemTool::new())).unwrap();
    let registry = Arc::new(registry);

    let kernel = Kernel::new(Arc::clone(&registry));
    let tool = kernel.tool_registry().get("filesystem").unwrap();
    let result = kernel
        .invoke_tool(
            tool.as_ref(),
            json!({"operation": "read", "path": file.to_string_lossy()}),
        )
        .await;
    assert!(result.is_err());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn duplicate_tool_registration_rejected() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(FilesystemTool::new())).unwrap();
    let result = registry.register(Arc::new(FilesystemTool::new()));
    assert!(result.is_err());
}

#[test]
fn ssrf_blocks_ipv6_ranges() {
    use std::net::{IpAddr, Ipv6Addr};

    assert!(picobot::tools::net_utils::is_private_ip(IpAddr::V6(
        Ipv6Addr::LOCALHOST
    )));
    assert!(picobot::tools::net_utils::is_private_ip(IpAddr::V6(
        Ipv6Addr::UNSPECIFIED
    )));
    assert!(picobot::tools::net_utils::is_private_ip(IpAddr::V6(
        Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)
    )));
    assert!(picobot::tools::net_utils::is_private_ip(IpAddr::V6(
        Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1)
    )));
    assert!(picobot::tools::net_utils::is_private_ip(IpAddr::V6(
        Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 1)
    )));
    assert!(picobot::tools::net_utils::is_private_ip(IpAddr::V6(
        Ipv6Addr::new(0, 0, 0, 0, 0, 0xffff, 0x7f00, 1)
    )));
}

#[test]
#[ignore = "enabled after http download limits are enforced"]
fn http_download_limits_enforced() {
    // Placeholder for regression coverage once streaming/body limits are added.
}

struct TestNotificationChannel;

#[async_trait]
impl NotificationChannel for TestNotificationChannel {
    fn channel_id(&self) -> &str {
        "test"
    }

    async fn send(&self, _request: NotificationRequest) -> Result<(), anyhow::Error> {
        Ok(())
    }
}

fn build_notifications() -> NotificationService {
    let config = NotificationQueueConfig::default();
    let queue = NotificationQueue::new(config);
    let channel = std::sync::Arc::new(TestNotificationChannel);
    NotificationService::new(queue, channel)
}

fn build_scheduler(temp_dir: &std::path::Path) -> SchedulerService {
    let store = picobot::session::db::SqliteStore::new(
        temp_dir.join("picobot.db").to_string_lossy().to_string(),
    );
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
    let mut scheduler_config = picobot::config::SchedulerConfig::default();
    scheduler_config.enabled = Some(true);
    let executor = JobExecutor::new(
        Arc::new(kernel),
        schedule_store.clone(),
        scheduler_config.clone(),
        agent_builder,
        None,
        picobot::config::Config::default(),
    );
    SchedulerService::new(schedule_store, executor, scheduler_config)
}

#[tokio::test]
async fn notify_rejects_cross_user_override() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(NotifyTool::new())).unwrap();
    let registry = Arc::new(registry);
    let mut capabilities = CapabilitySet::empty();
    capabilities.insert(Permission::Notify {
        channel: "*".to_string(),
    });
    let kernel = Kernel::new(Arc::clone(&registry))
        .with_capabilities(capabilities)
        .with_notifications(Some(Arc::new(build_notifications())))
        .with_channel_id(Some("repl".to_string()))
        .clone_with_context(Some("alice".to_string()), Some("repl:session".to_string()));
    let tool = kernel.tool_registry().get("notify").unwrap();
    let result = kernel
        .invoke_tool(
            tool.as_ref(),
            json!({"message": "hi", "user_id": "bob", "channel_id": "repl"}),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn notify_rejects_cross_channel_override() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(NotifyTool::new())).unwrap();
    let registry = Arc::new(registry);
    let mut capabilities = CapabilitySet::empty();
    capabilities.insert(Permission::Notify {
        channel: "*".to_string(),
    });
    let kernel = Kernel::new(Arc::clone(&registry))
        .with_capabilities(capabilities)
        .with_notifications(Some(Arc::new(build_notifications())))
        .with_channel_id(Some("repl".to_string()))
        .clone_with_context(Some("alice".to_string()), Some("repl:session".to_string()));
    let tool = kernel.tool_registry().get("notify").unwrap();
    let result = kernel
        .invoke_tool(
            tool.as_ref(),
            json!({"message": "hi", "channel_id": "whatsapp"}),
        )
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn schedule_rejects_cross_user_override() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ScheduleTool::new())).unwrap();
    let registry = Arc::new(registry);
    let dir = std::env::temp_dir().join(format!("picobot-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let scheduler = Arc::new(build_scheduler(&dir));
    let mut capabilities = CapabilitySet::empty();
    capabilities.insert(Permission::Schedule {
        action: "create".to_string(),
    });
    let kernel = Kernel::new(Arc::clone(&registry))
        .with_capabilities(capabilities)
        .with_scheduler(Some(scheduler))
        .with_channel_id(Some("repl".to_string()))
        .clone_with_context(Some("alice".to_string()), Some("repl:session".to_string()));
    let tool = kernel.tool_registry().get("schedule").unwrap();
    let result = kernel
        .invoke_tool(
            tool.as_ref(),
            json!({
                "action": "create",
                "schedule_type": "interval",
                "schedule_expr": "60",
                "task_prompt": "ping",
                "user_id": "bob"
            }),
        )
        .await;
    std::fs::remove_dir_all(&dir).ok();
    assert!(result.is_err());
}

#[tokio::test]
async fn notify_requires_permission() {
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(NotifyTool::new())).unwrap();
    let registry = Arc::new(registry);
    let kernel = Kernel::new(Arc::clone(&registry))
        .with_notifications(Some(Arc::new(build_notifications())))
        .with_channel_id(Some("repl".to_string()))
        .clone_with_context(Some("alice".to_string()), Some("repl:session".to_string()));
    let tool = kernel.tool_registry().get("notify").unwrap();
    let result = kernel
        .invoke_tool(tool.as_ref(), json!({"message": "hi"}))
        .await;
    assert!(result.is_err());
}
