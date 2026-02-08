use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt};

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

async fn run_command(
    mut cmd: tokio::process::Command,
    limits: &ExecutionLimits,
) -> Result<ShellOutput, ToolError> {
    cmd.kill_on_drop(true);
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| ToolError::new(err.to_string()))?;

    let stdout_handle = child.stdout.take().map(|stdout| {
        tokio::spawn(read_stream_limited(stdout, limits.max_output_bytes))
    });
    let stderr_handle = child.stderr.take().map(|stderr| {
        tokio::spawn(read_stream_limited(stderr, limits.max_output_bytes))
    });

    let status = tokio::time::timeout(limits.timeout, child.wait()).await;
    match status {
        Ok(result) => {
            let status = result.map_err(|err| ToolError::new(err.to_string()))?;
            let (stdout, stdout_truncated) = collect_output(stdout_handle).await?;
            let (stderr, stderr_truncated) = collect_output(stderr_handle).await?;
            let truncated = stdout_truncated || stderr_truncated;
            Ok(ShellOutput {
                exit_code: status.code(),
                stdout: render_output(stdout, stdout_truncated),
                stderr: render_output(stderr, stderr_truncated),
                timed_out: false,
                truncated,
            })
        }
        Err(_) => {
            let _ = child.kill().await;
            let _ = child.wait().await;
            if let Some(handle) = stdout_handle {
                handle.abort();
            }
            if let Some(handle) = stderr_handle {
                handle.abort();
            }
            Ok(ShellOutput {
                exit_code: None,
                stdout: String::new(),
                stderr: String::new(),
                timed_out: true,
                truncated: false,
            })
        }
    }
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
        limits: &ExecutionLimits,
    ) -> Result<ShellOutput, ToolError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.current_dir(working_dir);
        for arg in args {
            cmd.arg(arg);
        }
        run_command(cmd, limits).await
    }
}

#[derive(Debug)]
pub struct ContainerRunner {
    runtime: String,
    image: String,
    mount_source: PathBuf,
    mount_target: PathBuf,
}

impl ContainerRunner {
    pub fn new(runtime: String, image: String, mount_source: PathBuf) -> Self {
        let mount_source = mount_source.canonicalize().unwrap_or(mount_source);
        Self {
            runtime,
            image,
            mount_source,
            mount_target: PathBuf::from("/workspace"),
        }
    }

    fn container_working_dir(&self, working_dir: &Path) -> Result<PathBuf, ToolError> {
        let relative = working_dir.strip_prefix(&self.mount_source).map_err(|_| {
            ToolError::new(format!(
                "working_dir '{}' is outside container root '{}'",
                working_dir.display(),
                self.mount_source.display()
            ))
        })?;
        Ok(self.mount_target.join(relative))
    }

    fn build_runtime_args(
        &self,
        command: &str,
        args: &[String],
        working_dir: &Path,
        limits: &ExecutionLimits,
    ) -> Result<Vec<String>, ToolError> {
        let container_working_dir = self.container_working_dir(working_dir)?;
        let mut runtime_args = Vec::new();
        runtime_args.push("run".to_string());
        runtime_args.push("--rm".to_string());
        runtime_args.push("--network=none".to_string());
        if let Some(max_memory_bytes) = limits.max_memory_bytes {
            runtime_args.push("--memory".to_string());
            runtime_args.push(max_memory_bytes.to_string());
        }
        runtime_args.push("-v".to_string());
        runtime_args.push(format!(
            "{}:{}",
            self.mount_source.display(),
            self.mount_target.display()
        ));
        runtime_args.push("-w".to_string());
        runtime_args.push(container_working_dir.display().to_string());
        runtime_args.push(self.image.clone());
        runtime_args.push(command.to_string());
        runtime_args.extend(args.iter().cloned());
        Ok(runtime_args)
    }
}

#[async_trait]
impl ShellRunner for ContainerRunner {
    async fn run(
        &self,
        command: &str,
        args: &[String],
        working_dir: &Path,
        limits: &ExecutionLimits,
    ) -> Result<ShellOutput, ToolError> {
        let runtime_args = self.build_runtime_args(command, args, working_dir, limits)?;
        let mut cmd = tokio::process::Command::new(&self.runtime);
        cmd.args(runtime_args);
        run_command(cmd, limits).await
    }
}

async fn collect_output(
    handle: Option<tokio::task::JoinHandle<Result<(Vec<u8>, bool), std::io::Error>>>,
) -> Result<(Vec<u8>, bool), ToolError> {
    if let Some(handle) = handle {
        match handle.await {
            Ok(Ok(output)) => Ok(output),
            Ok(Err(err)) => Err(ToolError::new(err.to_string())),
            Err(err) => Err(ToolError::new(err.to_string())),
        }
    } else {
        Ok((Vec::new(), false))
    }
}

async fn read_stream_limited<R: AsyncRead + Unpin>(
    mut reader: R,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool), std::io::Error> {
    let mut buffer = Vec::new();
    let mut truncated = false;
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        if max_bytes == 0 {
            truncated = true;
            continue;
        }
        let available = max_bytes.saturating_sub(buffer.len());
        if available > 0 {
            let take = available.min(read);
            buffer.extend_from_slice(&chunk[..take]);
        }
        if read > available {
            truncated = true;
        }
    }
    Ok((buffer, truncated))
}

fn render_output(bytes: Vec<u8>, truncated: bool) -> String {
    let mut output = String::from_utf8_lossy(&bytes).to_string();
    if truncated && !output.is_empty() {
        output.push_str("\n\n[truncated]");
    }
    output
}
