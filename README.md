# PicoBot

Minimal agent runner built on rig-core.

## Quickstart

1. Set an API key in your environment:

```bash
export OPENAI_API_KEY="your-key"
```

2. Create a config file:

```bash
cp config.example.toml picobot.toml
```

3. Run the REPL:

```bash
cargo run
```

To persist schedule ownership, set a user/session id for the REPL:

```bash
PICOBOT_USER_ID=local-user PICOBOT_SESSION_ID=repl:local cargo run
```

4. Run the API server:

```bash
cargo run -- api
```

5. List or cancel scheduled jobs:

```bash
cargo run -- schedules list <user_id> [session_id]
cargo run -- schedules cancel <job_id>
```

## Config

Configuration defaults to `picobot.toml` in the repo root. You can override the path with `PICOBOT_CONFIG`.

```bash
PICOBOT_CONFIG=./my-config.toml cargo run
```

Use `picobot.example.toml` as a template for OpenAI, OpenRouter, or Gemini.

### Multi-Model Support & Routing

You can define multiple named model configurations and set a default model for routing.

```toml
# Top-level defaults
provider = "openai"
model = "gpt-4o-mini"

[[models]]
id = "fast"
provider = "openai"
model = "gpt-4o-mini"
max_turns = 10

[[models]]
id = "router"
provider = "openrouter"
model = "openai/gpt-4o-mini"
api_key_env = "OPENROUTER_API_KEY"

[routing]
default_model = "fast"
```

### Channels & Permissions

Each channel (e.g., `repl`, `api`) can have specific permission profiles. Permissions are strings like `filesystem:read:/path/**`, `shell:git`, or `memory:read:session`.

```toml
[channels.profiles.repl]
# Permissions granted automatically without user confirmation
pre_authorized = ["memory:read:session", "memory:write:session"]

# Maximum permissions a user can be granted in this channel (via prompts)
max_allowed = ["filesystem:read:./data/**", "shell:git", "schedule:*"]

# Whether to allow human-in-the-loop prompts for permissions in 'max_allowed'
allow_user_prompts = true

# Timeout for user confirmation prompts
prompt_timeout_secs = 60
```

### Defaults & Optional Settings

Most configuration options are optional and have sane defaults:

| Option | Default | Description |
| --- | --- | --- |
| `provider` | `openai` | AI provider for the single-model config |
| `model` | `gpt-4o-mini` | Model name for the single-model config |
| `system_prompt` | `You are PicoBot, a helpful assistant.` | Assistant preamble |
| `max_turns` | `5` | Max tool-calling iterations |
| `bind` | `127.0.0.1:8080` | API server address |
| `data_dir` | OS data dir + `picobot` | Base directory for storage |
| `scheduler.enabled` | `false` | Enable background job runner |
| `channels.allow_user_prompts` | `true` | Allow interactive permission requests |
| `channels.prompt_timeout_secs` | `30` | Seconds to wait for user response |

### Scheduler

Enable scheduling with a permission allowlist for schedule actions:

```toml
[scheduler]
enabled = true

[permissions.schedule]
allowed_actions = ["create", "list", "cancel"]
```

Note: scheduling requires `schedule:*` permissions, which can be granted globally via `permissions.schedule.allowed_actions` or per-channel via `channels.profiles.<channel>.max_allowed`.
