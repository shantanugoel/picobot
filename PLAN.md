# PicoBot - Roadmap

## Phase 1: Communication Surface

- WhatsApp adapter (send/receive, auth, session lifecycle)
- We should be able to run the agent as a server so it can communicate with whatsapp always on, and should also be able to connect to it via TUI simultaneously if needed
- Maybe we evaluate building HTTP API (REST + WebSocket) for core?
- Webhook retries + delivery observability
- Is an admin interface needed?

## Phase 2: Persistent Memory

- SQLite-backed conversation store
- Memory retrieval hooks in agent loop
- Privacy controls (retention window + purge command)

## Phase 3: Heartbeats & Scheduling

- Job scheduler (cron + fixed-interval)
- Heartbeat tasks wired into agent loop
- Execution caps (max tasks per window)

## Phase 4: Security Hardening

- Audit log writer + rotation
- Sandboxed shell execution
- Resource limits (CPU, memory, wall time)
- Capability grants lifecycle (expiry + revocation)
- Secure Admin TUI, accessible only via a specific port different from regular chat so we can avoid exposing it.

## Phase 5: Extensibility

- Dynamic tool loading (WASM plugins)
- Multi-model routing by task hint
- Tool metadata registry for discovery

## Phase 6

- Add a browser-use agent that can do extensive usage of browser
- Allow to persist cookies etc after logging in so it retains login status
- Think about how to get initial logins done for tricky cases (captchas that it can solve on its own, and where it can take user's help either via TUI or whatsapp, and where it needs user to login separately on their own and provide something to the bot)

## Phase 7: Ecosystem & UX

- Structured prompt templates for common tasks
- Export/import for config and profiles
- Operational docs and deployment guides
