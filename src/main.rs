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
use crate::tools::registry::ToolRegistry;

fn build_kernel(config: &Config) -> Result<Kernel> {
    let mut registry = ToolRegistry::new();
    registry.register(std::sync::Arc::new(FilesystemTool::new()))?;
    let capabilities = CapabilitySet::from_config(&config.permissions());
    let jail_root = config
        .permissions()
        .filesystem
        .and_then(|filesystem| filesystem.jail_root)
        .map(|path| {
            let expanded = if path.starts_with('~') {
                if path == "~" || path.starts_with("~/") {
                    if let Some(home) = dirs::home_dir() {
                        let trimmed = path.trim_start_matches('~');
                        home.join(trimmed.trim_start_matches('/'))
                            .to_string_lossy()
                            .to_string()
                    } else {
                        path
                    }
                } else {
                    path
                }
            } else {
                path
            };
            std::path::PathBuf::from(expanded)
        });
    let kernel = Kernel::new(registry)
        .with_capabilities(capabilities)
        .with_working_dir(config.data_dir())
        .with_jail_root(jail_root);
    Ok(kernel)
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
