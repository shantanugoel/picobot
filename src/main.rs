mod channels;
mod config;
mod kernel;
mod providers;
mod tools;

use anyhow::Result;

use crate::channels::{api, repl};
use crate::config::Config;
use crate::kernel::kernel::Kernel;
use crate::kernel::permissions::CapabilitySet;
use crate::tools::filesystem::FilesystemTool;
use crate::tools::http::HttpTool;
use crate::tools::schedule::ScheduleTool;
use crate::tools::shell::ShellTool;
use crate::tools::registry::ToolRegistry;

fn build_kernel(config: &Config) -> Result<Kernel> {
    let mut registry = ToolRegistry::new();
    registry.register(std::sync::Arc::new(FilesystemTool::new()))?;
    registry.register(std::sync::Arc::new(ShellTool::new()))?;
    registry.register(std::sync::Arc::new(HttpTool::new()?))?;
    registry.register(std::sync::Arc::new(ScheduleTool::new()))?;
    let base_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let capabilities = CapabilitySet::from_config_with_base(&config.permissions(), &base_dir);
    let jail_root = config
        .permissions()
        .filesystem
        .and_then(|filesystem| filesystem.jail_root)
        .map(|path| resolve_working_path(&base_dir, &path));
    let kernel = Kernel::new(registry)
        .with_capabilities(capabilities)
        .with_working_dir(resolve_working_path(
            &base_dir,
            &config.data_dir().to_string_lossy(),
        ))
        .with_jail_root(jail_root);
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
    let config = Config::load()?;
    let kernel = build_kernel(&config)?;

    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map(|arg| arg.as_str()).unwrap_or("repl");

    match mode {
        "api" => api::serve(config, kernel).await,
        "repl" => repl::run(config, kernel).await,
        other => {
            eprintln!("unknown mode '{other}', use 'repl' or 'api'");
            Ok(())
        }
    }
}
