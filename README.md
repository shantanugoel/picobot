# PicoBot

Minimal agent runner built on rig-core.

## Quick Start (Minimal)

1. Set your API key:

```bash
export OPENAI_API_KEY="your-key"
```

2. Run the REPL:

```bash
cargo run
```

That uses the default provider/model (`openai` / `gpt-4o-mini`) with defaults for everything else.

## Quickstart (With Config)

1. Copy the example config:

```bash
cp picobot.example.toml picobot.toml
```

2. Run the REPL:

```bash
cargo run
```

3. Run the API server:

```bash
cargo run -- api
```

4. Run WhatsApp (after enabling it in config):

```bash
cargo run -- whatsapp
```

5. List or cancel scheduled jobs:

```bash
cargo run -- schedules list <user_id> [session_id]
cargo run -- schedules cancel <job_id>
```

To persist schedule ownership in the REPL:

```bash
PICOBOT_USER_ID=local-user PICOBOT_SESSION_ID=repl:local cargo run
```

## Config

Defaults to `picobot.toml` in the repo root. Override with `PICOBOT_CONFIG`.

```bash
PICOBOT_CONFIG=./my-config.toml cargo run
```

Use `picobot.example.toml` as a template. Options marked "Optional" have defaults.

### Core Options

| Option | Default | Required? | Notes |
| --- | --- | --- | --- |
| `provider` | `openai` | Optional | `openai`, `openrouter`, `gemini` |
| `model` | `gpt-4o-mini` | Optional | Model name for selected provider |
| `system_prompt` | Security-hardened tool-first prompt | Optional | Assistant preamble (see `picobot.example.toml`) |
| `max_turns` | `5` | Optional | Max tool-calling iterations |
| `bind` | `127.0.0.1:8080` | Optional | API server bind address |
| `data_dir` | OS data dir + `picobot` | Optional | Base path for data/storage |
| `base_url` | provider default | Optional | Custom base URL (OpenAI-compatible) |
| `api_key_env` | provider default | Optional | Env var containing API key |

### Multi-Model Routing (Optional)

```toml
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

### Permissions (Optional)

PicoBot follows a default-deny model: tools and resources are only accessible if explicitly allowlisted. Global permissions serve as defaults for all channels.

```toml
[permissions.filesystem]
read_paths = ["./data/**"]
write_paths = ["./data/**"]
jail_root = "./data"

[permissions.network]
allowed_domains = ["api.github.com"]
# max_response_bytes = 5242880
# max_response_chars = 50000

[permissions.shell]
allowed_commands = ["git", "rg"]

[permissions.schedule]
allowed_actions = ["create", "list", "cancel"]
```

Notes:
- If `[permissions]` is omitted in `picobot.toml`, file/network/shell/schedule are denied.
- Memory permissions for session/user are auto-granted when those IDs are present in the tool context.

### Scheduler (Optional)

```toml
[scheduler]
enabled = true
tick_interval_secs = 1
max_concurrent_jobs = 4
max_concurrent_per_user = 2
max_jobs_per_user = 50
max_jobs_per_window = 100
window_duration_secs = 3600
job_timeout_secs = 300
max_backoff_secs = 3600
```

### Notifications (Optional)

```toml
[notifications]
enabled = false
max_attempts = 3
base_backoff_ms = 200
max_backoff_ms = 5000
```

Notes:
- The `notify` tool requires channel permissions (see channel profiles below).
- Notifications are only delivered for channels with a notification backend (currently WhatsApp).

### Memory (Optional)

```toml
[memory]
enable_user_memories = true
context_budget_tokens = 4000
max_session_messages = 50
max_user_memories = 50
include_summary_on_truncation = true
include_tool_messages = true
```

### Channels & Permission Profiles (Optional)

Each channel can override permissions and prompt settings. If a channel has no profile, it uses the default pre-authorized set (session memory + notify). Identity is bound to the current context; notify/schedule calls cannot override `user_id` or `channel_id` unless running in system/admin mode.

```toml
[channels.profiles.repl]
pre_authorized = ["memory:read:session", "memory:write:session"]
max_allowed = ["filesystem:read:./data/**", "shell:git", "schedule:*"]
allow_user_prompts = true
prompt_timeout_secs = 60

[channels.profiles.api]
pre_authorized = ["memory:read:session", "memory:write:session"]
allow_user_prompts = false

[channels.profiles.whatsapp]
pre_authorized = ["memory:read:session", "memory:write:session", "notify:whatsapp"]
allow_user_prompts = false
```

### WhatsApp (Optional)

```toml
[whatsapp]
enabled = false
store_path = "./data/whatsapp.db"
allowed_senders = ["15551234567@c.us"]
max_concurrent_messages = 10
max_media_size_bytes = 10485760
media_retention_hours = 24
```

Notes:
- `allowed_senders` must be WhatsApp JIDs (e.g., `15551234567@c.us`).
- Media is downloaded into a local staging directory under `data_dir/whatsapp-media/` and exposed to the agent via file paths.

### Multimodal Looker Tool (Optional)

The `multimodal_looker` tool analyzes local files or URLs for images, audio, video, and documents using a multimodal-capable model.

```toml
[multimodal]
# Use a model by id from [[models]] (recommended when routing is configured)
# model_id = "multimodal"

# Or specify provider/model directly
# provider = "openai"
# model = "gpt-4o"
# base_url = "https://api.openai.com/v1"
# api_key_env = "OPENAI_API_KEY"
# system_prompt = "You are a helpful multimodal assistant."
# max_media_size_bytes = 20971520
# max_image_size_bytes = 10485760
```

Notes:
- URLs require `permissions.network.allowed_domains` to permit the host.
- If `[multimodal]` is not set, the tool falls back to the main provider/model.
- `[vision]` is accepted as a backward-compatible alias for `[multimodal]`.

### Web Search Tool (Optional)

The `web_search` tool queries a configured search provider and returns a short list of results for the assistant to inspect.

Google Custom Search example:

```toml
[search]
provider = "google"
api_key_env = "GOOGLE_CSE_API_KEY"
engine_id = "your-search-engine-id"
max_results = 5
max_snippet_chars = 2000

[permissions.network]
allowed_domains = ["www.googleapis.com"]
```

SearxNG example (with fallbacks):

```toml
[search]
provider = "searxng"
base_urls = [
  "https://searx.rhscz.eu",
  "https://searxng.example.com"
]
allow_private_base_urls = false
searxng_engines = "google,duckduckgo"
searxng_categories = "general"
searxng_safesearch = 1
max_results = 5
max_snippet_chars = 2000

[permissions.network]
allowed_domains = ["searx.rhscz.eu", "searxng.example.com"]
```

Notes:
- Google requires a Programmable Search Engine id (`engine_id` / "cx") and an API key.
- SearxNG needs at least one `base_url` or `base_urls` entry.
- Set `allow_private_base_urls = true` if your SearxNG instance is on a private LAN or localhost.
- The tool only returns metadata. Use `http_fetch` for full page content.

## Environment Variables

| Variable | Purpose |
| --- | --- |
| `OPENAI_API_KEY` | OpenAI API key (default) |
| `OPENROUTER_API_KEY` | OpenRouter API key |
| `GEMINI_API_KEY` | Gemini API key |
| `PICOBOT_USER_ID` | REPL user id |
| `PICOBOT_SESSION_ID` | REPL session id |
| `PICOBOT_CONFIG` | Path to config file |
| `GOOGLE_CSE_API_KEY` | Google Custom Search API key |
