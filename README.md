# PicoBot

PicoBot is a security-first AI agent with a kernel that enforces capability checks on every tool invocation. It ships with a TUI, multi-provider models (via `genai`), and a minimal toolset (filesystem, shell, HTTP fetch).

## Quick Start

### Standalone TUI

```bash
# Copy example config and update values
cp config.example.toml config.toml
# Set API keys in your environment (e.g., OPENAI_API_KEY, GEMINI_API_KEY)
cargo run
```

### Server + Remote TUI (WebSocket)

The server requires an API key when `server.auth.api_keys` is configured. The TUI can connect over WebSocket with the same API key.

```bash
# terminal 1
cargo run -- serve

# terminal 2
PICOBOT_WS_URL=ws://127.0.0.1:8080/ws \
PICOBOT_WS_API_KEY=change-me \
cargo run
```

## WhatsApp Setup

PicoBot supports WhatsApp via the native `whatsapp-rust` client (not the Meta Cloud API).

### Prerequisites

- Rust nightly toolchain pinned for this repo:

```bash
rustup override set nightly
```

### Configuration

Enable the WhatsApp channel in `config.toml` (see `config.example.toml`):

```toml
[channels.whatsapp]
enabled = true
store_path = "./data/whatsapp.db"
allowed_senders = ["919876543210@c.us"]
allow_user_prompts = false
pre_authorized = []
max_allowed = ["filesystem:read:/tmp/**"]
```

### Pairing (QR code)

1. Start the server:

```bash
cargo run -- serve
```

2. Start the TUI and connect over WebSocket:

```bash
PICOBOT_WS_URL=ws://127.0.0.1:8080/ws \
PICOBOT_WS_API_KEY=change-me \
cargo run
```

3. When pairing is required, a QR code appears in the TUI. Scan it in WhatsApp:
Settings > Linked Devices > Link a Device.

The session is stored at `store_path` and reused on restart.

### JID format

- Individual: `<phone>@c.us` (example India: `919876543210@c.us`)
- Group: `<group_id>@g.us`

## Usage

- Type text to chat.
- Built-in commands: `/help`, `/clear`, `/permissions`, `/models`, `/quit`.
- Purge commands: `/purge_session`, `/purge_user`, `/purge_older <days>` (TUI confirmation required).
- Permission prompts appear when a tool needs access outside the current capability set.

## Configuration Reference

PicoBot is configured via `config.toml`. Key options include:

### Server (`[server]`)

- `bind`: Network address to bind to (e.g., `127.0.0.1:8080`).
- `expose_externally`: When true, allows binding to non-localhost addresses.
- `auth.api_keys`: List of keys for REST/WS authentication.
- `cors.allowed_origins`: Allowed origins for browser clients.
- `rate_limit.requests_per_minute`: Request ceiling per minute.
- `rate_limit.per_key`: If true, applies limits per API key or user identity.

### Channels (`[channels.api|websocket|whatsapp]`)

- `enabled`: Toggle the channel.
- `store_path`: (WhatsApp only) Local path for the session database.
- `allowed_senders`: (WhatsApp only) JIDs allowed to message the bot.
- `allow_user_prompts`: Allow interactive permission prompts on this channel.
- `pre_authorized`: Capabilities granted by default.
- `max_allowed`: Hard limit on capabilities this channel can ever access.

### Scheduler (`[scheduler]`)

- `enabled`: Enable the scheduler loop (default false).
- `tick_interval_secs`: Poll cadence for due jobs.
- `max_concurrent_jobs`: Global in-flight job limit.
- `max_concurrent_per_user`: Per-user in-flight job limit.
- `max_jobs_per_user`: Max schedules per user.
- `max_jobs_per_window`: Max schedule creations per user within window.
- `window_duration_secs`: Quota window length in seconds.
- `job_timeout_secs`: Per-job execution timeout.
- `max_backoff_secs`: Max exponential backoff between retries.

### Notifications (`[notifications]`)

- `enabled`: Enable async notifications.
- `max_attempts`: Max delivery attempts per notification.
- `base_backoff_ms`: Initial retry backoff.
- `max_backoff_ms`: Max retry backoff.

### Heartbeats (`[heartbeats]`)

- `enabled`: Enable startup heartbeats registration.
- `default_interval_secs`: Default interval for heartbeat prompts.
- `heartbeats.prompts`: List of heartbeat definitions.
- `heartbeats.prompts.name`: Unique name for the heartbeat.
- `heartbeats.prompts.prompt`: Prompt executed on each run.
- `heartbeats.prompts.interval_secs`: Interval schedule in seconds.
- `heartbeats.prompts.cron`: Cron schedule expression.
- `heartbeats.prompts.timezone`: Timezone for cron schedules (defaults to UTC).

### Permissions (`[permissions]`)

- `filesystem.read_paths`: Globbed paths the filesystem tool can read.
- `filesystem.write_paths`: Globbed paths the filesystem tool can write.
- `network.allowed_domains`: Domains allowed for HTTP fetch.
- `shell.allowed_commands`: Shell commands allowed for execution.
- `shell.working_directory`: Default working directory for shell tool.

### Metrics

- `/metrics`: Prometheus-style metrics for sessions, deliveries, and scheduler jobs.

### Models & Routing

- `models`: List of model providers and IDs (providers map to `genai` adapters).
- `routing.default`: Default model ID.

#### Provider notes

- `openai`: OpenAI chat completions (uses `OPENAI_API_KEY` by default).
- `openrouter`: OpenAI-compatible endpoint (uses `OPENROUTER_API_KEY` by default; set `base_url` if needed).
- `anthropic`: Anthropic native API (uses `ANTHROPIC_API_KEY`).
- `gemini` or `google`: Gemini native API (uses `GEMINI_API_KEY`).
- `ollama`: Local Ollama (no API key required; default base URL `http://localhost:11434`).

### Sessions

- `session.snapshot_interval_secs`: How often to write session snapshots.
- `session.snapshot_path`: Snapshot file path.
- `session.retention.max_age_days`: Max age for message retention (default 90 days).
- `session.retention.cleanup_interval_secs`: Retention cleanup interval.
- `session.memory.enable_user_memories`: Toggle user-scoped memories.
- `session.memory.context_budget_tokens`: Memory context token budget.
- `session.memory.max_session_messages`: Max session messages in context.
- `session.memory.max_user_memories`: Max user memories to include.
- `session.memory.enable_summarization`: Toggle summarization task.
- `session.memory.include_summary_on_truncation`: Inject summaries when truncating.
- `session.memory.summarization_trigger_tokens`: Threshold for summarization.

### Data

- `data.dir`: Base directory for persistent data (defaults to `data/`).

### Agent

- `agent.name`: Display name for the assistant.
- `agent.system_prompt`: System prompt prepended to conversations.

### Logging

- `logging.level`: Log level (e.g., `info`, `debug`).
- `logging.audit_file`: Audit log file path.

## Notes

- The kernel enforces permissions; tools only declare requirements.
- Tool output is treated as untrusted data and wrapped before re-entering the model.
- Scheduler metrics are exposed on `/metrics` along with session and delivery metrics.

## Development

```bash
cargo check
cargo clippy
cargo test
```
