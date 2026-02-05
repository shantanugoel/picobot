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
PICOBOT_CONFIG=./picobot.toml cargo run
```

Use `config.example.toml` as a starting point for OpenAI, OpenRouter, or Gemini.

### Scheduler

Enable scheduling with a permission allowlist for schedule actions:

```toml
[scheduler]
enabled = true

[permissions.schedule]
allowed_actions = ["create", "list", "cancel"]
```

Note: scheduling requires `schedule:*` permissions via `permissions.schedule.allowed_actions`.
