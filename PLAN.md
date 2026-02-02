# PicoBot - Roadmap

## Phase 1: Communication Surface

- WhatsApp adapter (send/receive, auth, session lifecycle)
- HTTP API (REST + WebSocket) for remote access
- Webhook retries + delivery observability

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

## Phase 5: Extensibility

- Dynamic tool loading (WASM plugins)
- Multi-model routing by task hint
- Tool metadata registry for discovery

## Phase 6: Ecosystem & UX

- Structured prompt templates for common tasks
- Export/import for config and profiles
- Operational docs and deployment guides
