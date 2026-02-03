# PicoBot Phase 1 Refactor Plan (WhatsApp via whatsapp-rust)

This document captures the finalized Phase 1 plan for expanding PicoBot into a multi-channel, server-first architecture with REST/WS APIs, WhatsApp integration, and concurrent TUI access. The WhatsApp backend is **whatsapp-rust** (not Meta Cloud API).

## Executive Summary

Phase 1 transforms PicoBot from a single-process TUI application into a multi-channel, server-first system with:

- HTTP API (REST + WebSocket)
- WhatsApp integration via `whatsapp-rust`
- Concurrent TUI access as a WebSocket client
- Delivery observability and retries

## Final Architecture Decisions

- Split adapter pattern: separate inbound vs outbound adapters
- Server-first agent loop, TUI as WebSocket client
- Tiered permissions (user-grantable vs admin-only)
- WhatsApp backend via `whatsapp-rust`, with pluggable backend trait
- In-memory sessions + JSON snapshots (SQLite in Phase 2)
- Security in Phase 1: API keys, CORS, localhost-only default

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                              PicoBot Server                                  │
│                                                                             │
│  ┌─────────────────────┐     ┌─────────────────────────────────────────┐   │
│  │   Inbound Adapters  │     │              Core Engine                │   │
│  │                     │     │                                         │   │
│  │  ┌───────────────┐  │     │  ┌─────────────┐   ┌───────────────┐   │   │
│  │  │ WS Adapter    │──┼────▶│  │   Session   │   │    Kernel     │   │   │
│  │  │ (TUI + API)   │  │     │  │   Manager   │──▶│ (enforcement) │   │   │
│  │  └───────────────┘  │     │  └─────────────┘   └───────────────┘   │   │
│  │                     │     │         │                  │           │   │
│  │  ┌───────────────┐  │     │         ▼                  ▼           │   │
│  │  │ WhatsApp      │──┼────▶│  ┌─────────────┐   ┌───────────────┐   │   │
│  │  │ Adapter       │  │     │  │ Agent Loop  │◀──│ Tool Registry │   │   │
│  │  └───────────────┘  │     │  │ (per session)│   └───────────────┘   │   │
│  │                     │     │  └─────────────┘                       │   │
│  └─────────────────────┘     │         │                              │   │
│                              │         ▼                              │   │
│  ┌─────────────────────┐     │  ┌─────────────┐                       │   │
│  │  Outbound Senders   │     │  │   Model     │                       │   │
│  │                     │     │  │  Registry   │                       │   │
│  │  ┌───────────────┐  │     │  └─────────────┘                       │   │
│  │  │ WS Broadcaster│◀─┼─────┴─────────────────────────────────────────┘   │
│  │  └───────────────┘  │                                                   │
│  │                     │     ┌─────────────────────────────────────────┐   │
│  │  ┌───────────────┐  │     │           Supporting Services           │   │
│  │  │ WhatsApp      │◀─┼─────│                                         │   │
│  │  │ Sender        │  │     │  ┌─────────────┐   ┌───────────────┐   │   │
│  │  └───────────────┘  │     │  │  Delivery   │   │  Permission   │   │   │
│  └─────────────────────┘     │  │   Queue     │   │    Store      │   │   │
│                              │  └─────────────┘   └───────────────┘   │   │
│                              └─────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Core Abstractions

### Split Adapter Pattern

```rust
#[async_trait]
pub trait InboundAdapter: Send + Sync {
    fn adapter_id(&self) -> &str;
    fn channel_type(&self) -> ChannelType;
    async fn subscribe(&self) -> Pin<Box<dyn Stream<Item = InboundMessage> + Send>>;
}

#[async_trait]
pub trait OutboundSender: Send + Sync {
    fn sender_id(&self) -> &str;
    fn supports_streaming(&self) -> bool;
    async fn send(&self, msg: OutboundMessage) -> Result<DeliveryId>;
    async fn stream_token(&self, session_id: &SessionId, token: &str) -> Result<()>;
}

pub struct Channel {
    pub id: String,
    pub channel_type: ChannelType,
    pub inbound: Arc<dyn InboundAdapter>,
    pub outbound: Arc<dyn OutboundSender>,
    pub permission_profile: PermissionProfile,
}
```

### Tiered Permission Model

```rust
pub enum PermissionTier {
    UserGrantable,
    AdminOnly,
}

pub struct PermissionProfile {
    pub pre_authorized: PermissionSet,
    pub max_allowed: PermissionSet,
    pub allow_user_prompts: bool,
    pub prompt_timeout_secs: u32,
}
```

### Session Management

```rust
pub struct Session {
    pub id: SessionId,
    pub channel_type: ChannelType,
    pub channel_id: String,
    pub user_id: String,
    pub conversation: Vec<Message>,
    pub permissions: PermissionSet,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    pub state: SessionState,
}

pub enum SessionState {
    Active,
    AwaitingPermission { tool: String, request_id: Uuid },
    Idle,
    Terminated,
}
```

## WhatsApp Integration (whatsapp-rust)

### Backend Choice

We will use the `whatsapp-rust` crate for WhatsApp integration. This is a Rust-native, async WhatsApp client with event handling and message send APIs. It requires session storage and QR-code pairing for initial auth.

### Backend Abstraction

```rust
#[async_trait]
pub trait WhatsAppBackend: Send + Sync {
    async fn start(&self) -> Result<()>;             // connect + event loop
    async fn send_text(&self, to: &str, body: &str) -> Result<DeliveryId>;
    fn inbound_stream(&self) -> Pin<Box<dyn Stream<Item = InboundMessage> + Send>>;
}
```

### whatsapp-rust Flow

- Use `Bot::builder()` with:
  - `SqliteStore` (or custom backend)
  - `TokioWebSocketTransportFactory`
  - `UreqHttpClient`
  - `on_event` handler for:
    - QR pairing (`Event::PairingQrCode`)
    - Incoming messages (`Event::Message`)
    - Receipts, errors
- On startup, prompt the user to scan QR code via TUI or CLI output
- Session lifecycle is managed by the whatsapp-rust backend (device store)

### Example Event Handling

```rust
match event {
    Event::PairingQrCode { code, .. } => {
        // forward QR to TUI or log
    }
    Event::Message(msg, info) => {
        // convert to InboundMessage
    }
    Event::Receipt(receipt) => {
        // map to delivery status
    }
    _ => {}
}
```

### Implications vs Cloud API

- Requires QR pairing, local session storage
- Higher ban risk than Cloud API
- More flexibility in message types and group handling
- No webhook flow; inbound messages are event-driven

## HTTP API Specification

```
POST   /api/v1/chat
POST   /api/v1/chat/stream
GET    /api/v1/sessions
GET    /api/v1/sessions/:id
DELETE /api/v1/sessions/:id

GET    /api/v1/permissions
POST   /api/v1/permissions/grant
DELETE /api/v1/permissions/:id

GET    /health
GET    /metrics
GET    /status
GET    /ws
```

## File Structure (Phase 1)

```
src/
├── main.rs
├── lib.rs
├── config.rs
├── kernel/           # existing
├── models/           # existing
├── tools/            # existing
├── channels/
│   ├── mod.rs
│   ├── adapter.rs
│   ├── websocket.rs
│   └── whatsapp.rs   # whatsapp-rust integration
├── server/
│   ├── mod.rs
│   ├── app.rs
│   ├── routes.rs
│   ├── ws.rs
│   ├── middleware.rs
│   └── state.rs
├── session/
│   ├── mod.rs
│   ├── manager.rs
│   └── snapshot.rs
├── delivery/
│   ├── mod.rs
│   ├── queue.rs
│   └── tracking.rs
├── observability/
│   ├── mod.rs
│   ├── metrics.rs
│   └── logging.rs
└── cli/
    └── tui.rs        # WS client
```

## Implementation Sequence

### Week 1: Core Foundation

- [x] Introduce inbound/outbound adapter traits
- [x] Build SessionManager + JSON snapshot persistence
- [x] Extend permissions with tiers
- [x] Adapter layer for agent_loop (preserve Kernel path)
- [x] Add channel permission profiles to config

### Week 2: HTTP Server + REST

- [x] Add axum + tower dependencies
- [x] Build AppState and REST endpoints
- [x] Add auth, CORS (rate limiting stubbed)
- [x] Implement chat endpoints (sync + stream)
- [x] Structured logging + health/metrics

### Week 3: WebSocket + TUI Migration

- [x] WebSocket protocol + adapter
- [x] Token streaming over WS
- [x] Permission flow over WS
- [x] TUI connects as WS client
- [x] Concurrent TUI + REST access

### Week 4: WhatsApp Integration (whatsapp-rust)

- Implement WhatsAppBackend trait
- Use whatsapp-rust Bot with SQLite store
- Handle QR pairing via TUI
- Convert inbound events to InboundMessage
- Map send_text to OutboundSender

### Week 5: Reliability + Observability

- Delivery queue with retry/backoff
- Delivery status tracking
- Prometheus metrics
- Integration tests for full flows

## Security Requirements (Phase 1)

- API key auth (required)
- CORS restrictions
- Localhost-only default binding
- Rate limiting (per-channel + per-user)
- Admin-only permissions cannot be granted via user channels

## Config Additions (Draft)

```toml
[server]
bind = "127.0.0.1:8080"
expose_externally = false

[server.auth]
api_keys = ["key1", "key2"]

[channels.whatsapp]
enabled = true
store_path = "./data/whatsapp.db"
allow_user_prompts = true
pre_authorized = ["http:read:*"]
max_allowed = ["http:*", "filesystem:read:/public/*"]

[session]
snapshot_interval_secs = 300
snapshot_path = "./data/sessions.json"
```

## Success Criteria

1. `picobot serve` runs server mode
2. REST API works for chat + sessions
3. WebSocket supports streaming + permissions
4. TUI connects via WS and retains full functionality
5. WhatsApp integration works via whatsapp-rust
6. Permissions enforced per channel
7. Delivery queue retries failed sends
8. Metrics available at `/metrics`
9. Session snapshots survive restart
