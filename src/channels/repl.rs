use std::io::{self, Write};
use std::sync::Arc;

use crate::config::Config;
use crate::kernel::kernel::Kernel;
use crate::providers::factory::ProviderFactory;
use anyhow::{Context, Result};

pub async fn run(config: Config, kernel: Kernel) -> Result<()> {
    let kernel = Arc::new(kernel);
    let agent = ProviderFactory::build_agent(&config, kernel.tool_registry(), kernel.clone())?;

    println!("picobot repl (type 'exit' to quit)");

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        print!("> ");
        stdout.flush().context("failed to flush stdout")?;

        let mut input = String::new();
        stdin
            .read_line(&mut input)
            .context("failed to read stdin")?;
        let prompt = input.trim();
        if prompt.is_empty() {
            continue;
        }
        if prompt == "exit" {
            break;
        }

        let response = agent
            .prompt_with_turns(prompt, config.max_turns())
            .await
            .context("prompt failed")?;
        println!("{response}");
    }

    Ok(())
}
