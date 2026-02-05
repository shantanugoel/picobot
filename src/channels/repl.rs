use std::io::{self, Write};
use std::sync::Arc;

use crate::config::Config;
use crate::kernel::core::Kernel;
use crate::providers::factory::ProviderAgentBuilder;
use anyhow::{Context, Result};

pub async fn run(
    config: Config,
    kernel: Kernel,
    agent_builder: ProviderAgentBuilder,
) -> Result<()> {
    let kernel = Arc::new(kernel);
    let agent = agent_builder.build(kernel.tool_registry(), kernel.clone(), config.max_turns());

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
