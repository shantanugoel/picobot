# PicoBot

PicoBot is a security-first AI agent with a kernel that enforces capability checks on every tool invocation. It ships with a TUI, OpenAI-compatible model providers, and a minimal toolset (filesystem, shell, HTTP fetch).

## Quick start

```bash
cp config.example.toml config.toml
# Set API keys in your environment
cargo run
```

## Server + TUI (WebSocket)

The server requires an API key when `server.auth.api_keys` is configured. The TUI can connect over WebSocket with the same API key.

```bash
# terminal 1
cargo run -- serve

# terminal 2
PICOBOT_WS_URL=ws://127.0.0.1:8080/ws \
PICOBOT_WS_API_KEY=change-me \
cargo run
```

## Usage

- Type text to chat.
- Built-in commands: `/help`, `/clear`, `/permissions`, `/models`, `/quit`.
- Permission prompts appear when a tool needs access outside the current capability set.

## Configuration

Update `config.toml` to change models, routing, and permissions. See `config.example.toml` for the full schema.

## Notes

- The kernel enforces permissions; tools only declare requirements.
- Tool output is treated as untrusted data and wrapped before re-entering the model.

## Development

```bash
cargo check
cargo clippy
cargo test
```
