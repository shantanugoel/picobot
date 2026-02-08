use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;

use crate::tools::traits::ToolError;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ExecutionLimits {
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub max_memory_bytes: Option<u64>,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            max_output_bytes: 1_048_576,
            max_memory_bytes: None,
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ShellOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
}

#[async_trait]
pub trait ShellRunner: Send + Sync + std::fmt::Debug {
    async fn run(
        &self,
        command: &str,
        args: &[String],
        working_dir: &Path,
        limits: &ExecutionLimits,
    ) -> Result<ShellOutput, ToolError>;
}

#[derive(Debug, Default)]
pub struct HostRunner;

#[async_trait]
impl ShellRunner for HostRunner {
    async fn run(
        &self,
        command: &str,
        args: &[String],
        working_dir: &Path,
        _limits: &ExecutionLimits,
    ) -> Result<ShellOutput, ToolError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.current_dir(working_dir);
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|err| ToolError::new(err.to_string()))?;

        Ok(ShellOutput {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            timed_out: false,
            truncated: false,
        })
    }
}
