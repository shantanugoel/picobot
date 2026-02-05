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
    let kernel = Kernel::new(registry)
        .with_capabilities(capabilities)
        .with_working_dir(config.data_dir());
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
