mod channels;
mod config;
mod kernel;
mod providers;
mod scheduler;
mod session;
mod tools;

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use crate::channels::{api, repl, whatsapp};
use crate::config::Config;
use crate::kernel::core::Kernel;
use crate::kernel::permissions::CapabilitySet;
use crate::providers::factory::{ProviderAgentBuilder, ProviderFactory};
use crate::tools::filesystem::FilesystemTool;
use crate::tools::http::HttpTool;
use crate::tools::memory::MemoryTool;
use crate::tools::registry::ToolRegistry;
use crate::tools::schedule::ScheduleTool;
use crate::tools::shell::ShellTool;

fn build_kernel(
    config: &Config,
    _agent_builder: ProviderAgentBuilder,
    scheduler: Option<std::sync::Arc<crate::scheduler::service::SchedulerService>>,
) -> Result<Kernel> {
    let mut registry = ToolRegistry::new();
    let session_store = crate::session::db::SqliteStore::new(
        config
            .data_dir()
            .join("sessions.db")
            .to_string_lossy()
            .to_string(),
    );
    session_store.touch()?;
    registry.register(std::sync::Arc::new(FilesystemTool::new()))?;
    registry.register(std::sync::Arc::new(ShellTool::new()))?;
    registry.register(std::sync::Arc::new(HttpTool::new()?))?;
    registry.register(std::sync::Arc::new(ScheduleTool::new()))?;
    registry.register(std::sync::Arc::new(MemoryTool::new(session_store.clone())))?;
    let registry = std::sync::Arc::new(registry);
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let capabilities = CapabilitySet::from_config_with_base(&config.permissions(), &base_dir);
    let jail_root = config
        .permissions()
        .filesystem
        .and_then(|filesystem| filesystem.jail_root)
        .map(|path| resolve_working_path(&base_dir, &path));
    let kernel = Kernel::new(std::sync::Arc::clone(&registry))
        .with_capabilities(capabilities)
        .with_working_dir(resolve_working_path(
            &base_dir,
            &config.data_dir().to_string_lossy(),
        ))
        .with_jail_root(jail_root)
        .with_scheduler(scheduler);
    Ok(kernel)
}

fn resolve_working_path(base_dir: &std::path::Path, raw: &str) -> std::path::PathBuf {
    let expanded = if raw == "~" || raw.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            let trimmed = raw.trim_start_matches('~');
            home.join(trimmed.trim_start_matches('/'))
        } else {
            std::path::PathBuf::from(raw)
        }
    } else {
        std::path::PathBuf::from(raw)
    };

    if expanded.is_absolute() {
        expanded
    } else {
        base_dir.join(expanded)
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
    let config = Config::load()?;
    let validation = config.validate()?;
    for warning in validation.warnings {
        tracing::warn!(warning = %warning, "config validation warning");
    }
    tracing::info!(
        provider = %config.provider(),
        model = %config.model(),
        max_turns = config.max_turns(),
        "config loaded"
    );
    let agent_builder = ProviderFactory::build_agent_builder(&config)?;
    let agent_router = ProviderFactory::build_agent_router(&config).ok();
    let kernel = build_kernel(&config, agent_builder.clone(), None)?;
    let scheduler = if config.scheduler().enabled() {
        let store = crate::session::db::SqliteStore::new(
            config
                .data_dir()
                .join("scheduler.db")
                .to_string_lossy()
                .to_string(),
        );
        store.touch()?;
        let schedule_store = crate::scheduler::store::ScheduleStore::new(store.clone());
        let executor = crate::scheduler::executor::JobExecutor::new(
            std::sync::Arc::new(kernel.clone()),
            schedule_store.clone(),
            config.scheduler(),
            agent_builder.clone(),
            agent_router.clone(),
            config.clone(),
        );
        let scheduler = crate::scheduler::service::SchedulerService::new(
            schedule_store,
            executor,
            config.scheduler(),
        );
        Some(std::sync::Arc::new(scheduler))
    } else {
        None
    };
    let kernel = kernel.with_scheduler(scheduler);

    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|arg| arg.as_str()).unwrap_or("repl");

    if let Some(scheduler) = kernel.context().scheduler.clone() {
        let runner = scheduler.clone();
        tokio::spawn(async move {
            runner.run_loop().await;
        });
    }

    match mode {
        "api" => api::serve(config, kernel, agent_builder.clone()).await,
        "repl" => repl::run(config, kernel, agent_builder.clone()).await,
        "whatsapp" => whatsapp::run(config, kernel, agent_builder.clone()).await,
        "schedules" => run_schedules_cli(&config, kernel, &args[2..]),
        other => {
            eprintln!("unknown mode '{other}', use 'repl', 'api', 'whatsapp', or 'schedules'");
            Ok(())
        }
    }
}

fn run_schedules_cli(_config: &Config, kernel: Kernel, args: &[String]) -> Result<()> {
    let Some(scheduler) = kernel.context().scheduler.clone() else {
        anyhow::bail!("scheduler is disabled; enable [scheduler].enabled = true in config");
    };
    match args.first().map(|value| value.as_str()).unwrap_or("help") {
        "list" => {
            let user_id = args
                .get(1)
                .cloned()
                .unwrap_or_else(|| "default-user".to_string());
            let jobs = if let Some(session_id) = args.get(2) {
                scheduler
                    .store()
                    .list_jobs_by_user_with_session(&user_id, session_id)?
            } else {
                scheduler.list_jobs_by_user(&user_id)?
            };
            if jobs.is_empty() {
                println!("no schedules for user '{user_id}'");
                return Ok(());
            }
            for job in jobs {
                println!(
                    "{} {} {:?} {}",
                    job.id, job.name, job.schedule_type, job.schedule_expr
                );
            }
            Ok(())
        }
        "cancel" => {
            let job_id = args
                .get(1)
                .ok_or_else(|| anyhow::anyhow!("missing job_id"))?;
            let cancelled = scheduler.cancel_job(job_id)?;
            println!("cancelled={cancelled}");
            Ok(())
        }
        _ => {
            println!("usage: cargo run -- schedules list <user_id> [session_id] | cancel <job_id>");
            Ok(())
        }
    }
}
