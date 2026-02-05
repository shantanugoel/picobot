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

4. Run the API server:

```bash
cargo run -- api
```

## Config

Configuration defaults to `picobot.toml` in the repo root. You can override the path with `PICOBOT_CONFIG`.

```bash
PICOBOT_CONFIG=./picobot.toml cargo run
```

Use `config.example.toml` as a starting point for OpenAI, OpenRouter, or Gemini.
