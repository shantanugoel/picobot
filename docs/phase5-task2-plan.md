# Phase 5 Task 2 Plan: Isolated Runtime + Tool Limits

## Goals
- Add a safe execution layer for high-risk tools (shell) without changing other tool behavior.
- Enforce timeouts and output limits to prevent hung or unbounded tool calls.
- Preserve Kernel as the single enforcement point.
- Keep containerization opt-in; default behavior remains host execution.

## Non-Goals
- Containerizing all tools (filesystem/http/multimodal/schedule/notify stay host-native).
- Changing tool schemas or ToolExecutor trait.
- Changing prompt or tool contract semantics.

## Design Overview

### Core idea
Introduce a `ShellRunner` abstraction used only by the shell tool. Provide:
- `HostRunner` (current behavior + timeouts/output truncation)
- `ContainerRunner` (OCI container exec, opt-in)

Separately, add per-tool timeouts at the Kernel boundary for all tools.

### High-level flow
```
Kernel.invoke_tool()
  -> schema validation
  -> permission check
  -> tool.execute() wrapped in timeout
       -> ShellTool.execute()
            -> ShellRunner.run(...)
```

## Detailed Plan

### 1) Add ShellRunner abstraction
**Files**
- New: `src/tools/shell_runner.rs`
- Update: `src/tools/shell.rs`

**API sketch**
```rust
pub struct ExecutionLimits {
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub max_memory_bytes: Option<u64>,
}

pub struct ShellOutput {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub truncated: bool,
}

#[async_trait]
pub trait ShellRunner: Send + Sync {
    async fn run(
        &self,
        command: &str,
        args: &[String],
        working_dir: &Path,
        limits: &ExecutionLimits,
    ) -> Result<ShellOutput, ToolError>;
}
```

**HostRunner**
- Extract current `tokio::process::Command` logic.
- Wrap in `tokio::time::timeout` using `ExecutionLimits.timeout`.
- Truncate stdout/stderr by `ExecutionLimits.max_output_bytes`.
- On timeout: kill child + wait to reap.

**ShellTool integration**
- ShellTool holds `Arc<dyn ShellRunner>`.
- Constructor injects runner (host by default).

### 2) Add per-tool timeout enforcement in Kernel
**Files**
- Update: `src/kernel/core.rs`
- Update: `src/config.rs`

**Behavior**
- Wrap `tool.execute()` with `tokio::time::timeout`.
- Use per-tool timeout from config (fallback to global default).
- Return typed timeout error.

**Note**
- Runner timeout uses the same limit passed from Kernel to avoid double-timeout confusion.

### 3) Add config for tool limits and shell runner
**Files**
- Update: `src/config.rs`
- Update: `picobot.example.toml`

**Config shape**
```toml
[permissions.tool_limits]
default_timeout_secs = 60
max_output_bytes = 1048576
shell_timeout_secs = 120
http_timeout_secs = 30
multimodal_timeout_secs = 120

[permissions.shell]
runner = "host"          # host | container
container_runtime = "docker"
container_image = "alpine:latest"
```

### 4) Add ContainerRunner (opt-in)
**Files**
- Update: `src/tools/shell_runner.rs`

**Behavior**
- `docker run --rm --network=none` for isolation.
- Bind-mount allowed root (prefer `jail_root`, fallback to `working_dir`).
- Set `-w` to mounted work dir.
- Apply memory/cpu limits if configured.
- Capture stdout/stderr with size cap.

**Fallback**
- If runtime binary not available: log warning and use HostRunner.
- Do not fail tool execution by default unless policy explicitly requires container.

### 5) Soft-timeout extension (optional)
**Goal**
- For interactive channels, request extension on timeout before hard stop.

**Approach**
- Reuse existing approval/prompt mechanism used by shell HITL.
- Soft timeout triggers prompt: extend or cancel.
- For non-interactive channels: follow policy (auto-deny or auto-extend up to cap).
- Always keep a hard cap to prevent infinite runs.

**Note**
- This can be implemented after the hard timeout baseline is in place.

### 6) User-scoped filesystem boundaries (recommended)
**Why**
- Prevent cross-user reads/writes when multiple users share a host.
- Make path-based permissions safe by default, even with broad allowlists.

**Affected tools**
- Filesystem, Shell (working_dir), Multimodal (local paths), any other tool using `resolve_path`.
- Network-only tools are unaffected.

**Behavior changes to plan for**
- Local paths must be under the per-user `jail_root`.
- WhatsApp media attachments must live under the user root (or jail_root must include the media root).
- Scheduled jobs should execute with the same user-root scope used at creation time.

**Approach**
- Compute a per-user root (e.g., `data/users/{user_id}`) and set both `working_dir` and `jail_root` on the Kernel for that user/session.
- Ensure channel entry points set `user_id`/`session_id` before building the Kernel clone.
- Store user-scoped media under the same user root (or explicitly expand jail_root to include media paths).
- For scheduled jobs, persist the user root or derive it deterministically from user_id at execution.

**Config sketch (optional)**
```toml
[permissions.filesystem]
user_scope = "user"  # none | user | session
user_root = "data/users/{user_id}"
```

## Sequencing (Low Risk to Higher Complexity)
1. Add `ShellRunner` + `HostRunner` (no behavior change).
2. Kernel timeout wrapper + config defaults.
3. Output truncation + kill-on-timeout in HostRunner.
4. ContainerRunner + config + runtime detection.
5. Soft-timeout extension flow (optional).
6. User-scoped filesystem boundaries (recommended, can be parallel once runner/timeout baseline lands).

## Compatibility & Safety Notes
- Only shell uses containerization to avoid breaking downloads/media tools.
- Keep Kernel as the single enforcement point for timeouts/permissions.
- Do not allow model to disable timeouts at runtime; only config can do it.
- Ensure `jail_root` or `working_dir` is mountable in container mode.
- If user scoping is enabled, all local-path tools must point inside the user root; update media storage paths accordingly.

## Testing Plan
- Unit: ShellRunner host timeout + output truncation.
- Integration: Kernel timeout errors for a known long-running command.
- Integration: ContainerRunner happy-path (if runtime available).
- Regression: filesystem/http/multimodal tools unchanged.
- Regression: user-scoped jail_root allows per-user media paths and blocks cross-user access.

## Success Criteria
- Shell tool can run in host mode exactly as before.
- Timeouts stop hung tools reliably with clean error reporting.
- Optional container mode works without breaking other tools/channels.
- User-scoped jail_root prevents cross-user local file access while keeping per-user media usable.
