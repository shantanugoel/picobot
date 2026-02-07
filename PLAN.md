# PicoBot Rewrite Plan: Rig-Core Migration

## Overview

This is a ground-up rewrite of PicoBot using `rig-core` as the foundation for AI provider abstraction, tool calling, and agent orchestration. The previous implementation (in `reference/`) used the `genai` crate and had issues with:

- Unreliable tool calling (duplications, failures)
- Message duplication and context interpretation errors
- Fragile message sanitization heuristics
- Pseudo-streaming (collected Vec instead of true async stream)

## Goals

1. **Reliable tool calling** via rig-core's structured tool handling
2. **True streaming** via rig-core's `stream_prompt()`
3. **Multi-provider support**: OpenAI, Anthropic, Gemini, OpenRouter (and ZAI GLM via OpenAI-compatible endpoint)
4. **Preserve security model**: Kernel-enforced permissions with allowlist-based capabilities
5. **Simpler codebase**: Eliminate ~1500 lines of custom orchestration code

## Architecture

```
┌───────────────────────────────────────────────────────────────────┐
│           Server Channels (API / WebSockets / WhatsApp)           │
└───────────────────────────────────────┬───────────────────────────┘
                                        │
                                        ▼
┌───────────────────────────────────────────────────────────────────┐
│                    Rig-Core Agent (Orchestrator)                  │
│  - Replaces reference/src/kernel/agent_loop.rs entirely           │
│  - multi_turn(n) for tool loops                                   │
│  - Built-in message sequencing                                    │
│  - True streaming via stream_prompt()                             │
└───────────────────────────────────────┬───────────────────────────┘
                                        │
         ┌──────────────────────────────┼──────────────────────────┐
         │                              │                          │
         ▼                              ▼                          ▼
┌─────────────────┐       ┌─────────────────┐       ┌─────────────────┐
│  OpenAI Client  │       │ Anthropic Client│       │  Gemini Client  │
│  (gpt-4, etc)   │       │ (Claude, etc)   │       │  (gemini-pro)   │
└─────────────────┘       └─────────────────┘       └─────────────────┘
         │                              │                          │
         └──────────────────────────────┼──────────────────────────┘
                                        │
                                        ▼
┌───────────────────────────────────────────────────────────────────┐
│                  KernelBackedTool (ToolDyn wrapper)               │
│  - Holds Arc<Kernel> reference                                    │
│  - Dynamic tool names via ToolDyn                                 │
│  - call() delegates to Kernel.invoke_tool()                       │
│  - Enforces CapabilitySet permissions                             │
└───────────────────────────────────────┬───────────────────────────┘
                                        │
                                        ▼
┌───────────────────────────────────────────────────────────────────┐
│                         Kernel                                    │
│  - Permission enforcement (CapabilitySet)                         │
│  - Canonicalized path checks with optional jail root              │
│  - Static capability enforcement                                  │
└───────────────────────────────────────────────────────────────────┘
```

## Reference Code

The previous implementation is preserved in `reference/` for guidance:

| Reference Path | Purpose | Notes |
|----------------|---------|-------|
| `reference/src/kernel/permissions.rs` | CapabilitySet, Permission types | Port directly, well-designed |
| `reference/src/kernel/agent.rs` | Kernel enforcement | Adapt for rig integration |
| `reference/src/tools/*.rs` | Tool implementations | Port with rig::Tool wrapper |
| `reference/src/tools/registry.rs` | Tool registry, schema validation | Simplify for rig |
| `reference/src/cli/tui.rs` | TUI implementation | Discard (migrating to Server Channels) |
| `reference/src/config.rs` | Configuration parsing | Port and simplify |
| `reference/src/kernel/memory.rs` | Memory/session management | Port for chat history |

**DO NOT PORT:**
- `reference/src/kernel/agent_loop.rs` - Replaced by rig-core's agent
- `reference/src/models/genai_adapter.rs` - Replaced by rig-core providers
- `reference/src/models/types.rs` - Use rig-core types instead
- `sanitize_messages()` logic - Rig handles message sequencing

## Implementation Phases

### Phase 1: Foundation (Week 1-2)

**Goal**: Minimal working agent with one provider and one tool.

#### Tasks

1. **Project Setup**
   - [x] Create `Cargo.toml` with rig-core and essential dependencies
   - [x] Create `src/main.rs` with basic structure
   - [x] Create `rust-toolchain.toml` (use stable, not nightly if possible)

2. **Kernel Core** (`src/kernel/`)
   - [x] Port `permissions.rs` (CapabilitySet, Permission, Scope types)
   - [x] Create `kernel.rs` with permission checking and tool invocation
   - [x] Create `session.rs` for session context (user/session IDs)

3. **Tool Infrastructure** (`src/tools/`)
    - [x] Create `traits.rs` with ToolSpec definition
    - [x] Create `rig_wrapper.rs` with KernelBackedTool implementation
    - [x] Port `filesystem.rs` as first tool (read/write/list)
    - [x] Create `registry.rs` for tool collection

4. **Provider Setup** (`src/providers/`)
    - [x] Create `factory.rs` for building rig-core clients
    - [x] Support OpenAI provider with configurable base_url
    - [x] Support OpenRouter provider (OpenAI-compatible)
    - [x] Create agent builder function

5. **Initial Interface** (`src/channels/`)
   - [x] Create minimal API endpoint for text prompts
   - [x] Wire up agent execution with tool calls
   - [x] Add basic REPL for local debugging

6. **Configuration** (`src/config.rs`)
    - [x] Port minimal config (model, permissions, data dir)
    - [x] Add optional `jail_root` for filesystem tools

#### Milestone: Run `cargo run`, chat with agent, execute filesystem tool

### Phase 2: Multi-Provider & Tools (Week 3-4)

**Goal**: Full provider support and complete tool set.

#### Tasks

1. **Additional Providers**
    - [x] Add Gemini client
    - [x] Create provider routing based on config

2. **Complete Tool Set**
    - [x] Port `shell.rs` (command execution)
    - [x] Port `http.rs` (HTTP fetch)
    - [x] Port `schedule.rs` (scheduling)
    - [x] Require allowlisted commands for shell tool

3. **Streaming Support**
   - [x] Implement streaming output to CLI
   - [x] Handle tool call events during stream

4. **Session & Memory**
    - [x] Port SQLite session storage
    - [x] Implement chat history management
    - [x] Port memory retrieval for context

5. **Configuration Expansion**
    - [x] Add multi-model configuration
    - [x] Add channel permissions (pre_authorized, max_allowed)
    - [x] Ask user (if allowed for channel via config, default enabled) for expanding permission 

#### Milestone: Switch between providers, all tools working, streaming output.

### Phase 3: WhatsApp & Robustness (Week 5-6)

**Goal**: Production-ready WhatsApp channel and core reliability.

#### Tasks

1. **WhatsApp Integration**
   - [x] Implement WhatsApp adapter (using whatsapp-rust. Take hint from reference)
   - [x] Handle media/document processing for tools
   - [x] Session management for concurrent users

2. **Robustness**
   - [x] Error handling and recovery
   - [x] Gracefully recover from upstream errors instead of crashing out
   - [x] Logging and audit trail
   - [x] Configuration validation

3. **Testing**
   - [x] Unit tests for Kernel
   - [x] Unit tests for permissions
   - [x] Integration tests for tool execution

4. **Documentation**
   - [ ] Update README.md for quick start guide, some other common example configurations, and a full configuration reference. Also update picobot.example.toml to be more complete

#### Milestone: Feature parity with reference implementation.

### Phase 4: Security Foundations, Access Control & Safety (Week 7-8)

**Goal**: Resolve highest-risk authz/cross-tenant issues first, then harden IO boundaries with regression coverage before advanced expansion.

#### Tasks (implementation sequence)

1. **Security Regression Harness & Audit Baseline**
   - [ ] Add security regression tests first (cross-user spoofing, cancel semantics, SSRF, download limits, duplicate tool registration). Files: `tests/kernel_integration.rs`, `tests/tool_execution_integration.rs`, `tests/scheduler_integration.rs`.
   - [ ] Structured audit logs for tool usage. Approach: include actor identity, channel/session, tool name, decision, and outcome. Files: `src/kernel/core.rs`, `src/channels/*`, `src/scheduler/executor.rs`.
   - [ ] Log permission denials for identity mismatch, SSRF blocks, and restricted notification attempts. Files: `src/kernel/core.rs`, `src/tools/net_utils.rs`, `src/tools/notify.rs`, `src/tools/schedule.rs`.
   - [ ] Log approval decisions with context for interactive channels. Files: `src/kernel/core.rs`, `src/channels/repl.rs`, `src/channels/permissions.rs`.

2. **Permission Boundary Hardening**
   - [ ] Bind `user_id`/`channel_id`/`session_id` to `ToolContext` for user-initiated calls (deny cross-user/channel overrides in tools like `schedule` and `notify`). Approach: reject identity overrides from tool input unless explicit system/admin execution mode is set. Files: `src/tools/schedule.rs`, `src/tools/notify.rs`, `src/kernel/core.rs`, `src/channels/api.rs`, `src/channels/repl.rs`, `src/channels/whatsapp.rs`.
   - [ ] Add explicit permission for `notify` in `CapabilitySet`/channel profiles (remove implicit always-allowed behavior). Approach: add notification permission type, parse from config, and enforce in Kernel before enqueueing notifications. Files: `src/kernel/permissions.rs`, `src/config.rs`, `src/channels/permissions.rs`, `src/tools/notify.rs`.

3. **Server Channel Hardening (Current API Surface)**
   - [ ] Evolve current Axum API from open `/prompt` into a versioned authenticated REST surface. Files: `src/channels/api.rs`, `src/main.rs`.
   - [ ] Add channel authentication/authorization and map authenticated identity into `ToolContext` (no client-controlled impersonation). Files: `src/channels/api.rs`, `src/kernel/core.rs`, `src/tools/traits.rs`.
   - [ ] Add per-channel rate limits and request body size limits. Files: `src/channels/api.rs`, `src/config.rs`.
   - [ ] Add secure schedule-management endpoints with owner checks and true cancel behavior. Files: `src/channels/api.rs`, `src/tools/schedule.rs`, `src/scheduler/service.rs`, `src/scheduler/store.rs`.
   - [ ] Add API integration tests for authz boundaries and anti-impersonation behavior. Files: new `tests/api_integration.rs`.

4. **Scheduler & Notification Safety**
   - [ ] Fix schedule cancel semantics so canceling a job also disables/deletes persisted schedule state (not only in-flight cancellation). Approach: owner-verified cancel updates persistent schedule state and optionally stops in-flight run. Files: `src/tools/schedule.rs`, `src/scheduler/service.rs`, `src/scheduler/store.rs`, `src/main.rs`.
   - [ ] Bound notification queue in-memory record growth (retention cap/TTL or persistence-backed pruning). Files: `src/notifications/queue.rs`, `src/notifications/service.rs`.

5. **Filesystem & Media Boundary Hardening**
   - [ ] Canonicalize/normalize all paths before checks. Approach: centralize path resolution and jail-root enforcement in one shared helper used by all file-touching tools. Files: `src/tools/path_utils.rs`, `src/tools/filesystem.rs`, `src/tools/multimodal_looker.rs`.
   - [ ] Re-check canonical paths at execution time for writes. Approach: repeat resolved path and jail checks right before write/open to reduce TOCTOU window. Files: `src/tools/filesystem.rs`.
   - [ ] Check if incoming media/documents from different users (WhatsApp/other channels) have leakage potential via filesystem/shell/other tools. Approach: enforce per-session/user media boundaries and avoid broad inherited read grants. Files: `src/channels/whatsapp.rs`, `src/tools/multimodal_looker.rs`, `src/tools/shell.rs`, `src/channels/permissions.rs`.
   - [ ] Remove duplicated path resolution logic and keep one canonical implementation. Files: `src/tools/filesystem.rs`, `src/tools/path_utils.rs`.

6. **Network Hardening & Response Controls**
   - [ ] Harden SSRF checks for IPv6 and non-global address ranges (loopback/link-local/ULA/etc). Approach: block all non-global resolved addresses and keep scheme/credential restrictions strict. Files: `src/tools/net_utils.rs`, `src/tools/http.rs`, `src/tools/multimodal_looker.rs`.
   - [ ] Add regression tests for DNS resolution safeguards across IPv4 + IPv6. Files: `src/tools/http.rs`, `src/tools/net_utils.rs`, `tests/tool_execution_integration.rs`.
   - [ ] Add response-size limits and streaming reads for network tools (`http_fetch`, `multimodal_looker`) to prevent unbounded memory use. Approach: consume response streams in chunks and abort after configured byte cap. Files: `src/tools/http.rs`, `src/tools/multimodal_looker.rs`, `src/config.rs`.

7. **Tooling Integrity & Runtime Hygiene**
   - [ ] Reject duplicate tool registrations in `ToolRegistry` (fail fast on duplicate names). Files: `src/tools/registry.rs`, `tests/tool_execution_integration.rs`.
   - [ ] Remove duplicated WhatsApp sender filtering path to keep one enforcement point and one audit trail. Files: `src/channels/whatsapp.rs`.
   - [ ] Resolve current dead-code warnings for security-relevant paths and diagnostics structs/traits. Files: `src/channels/whatsapp.rs`, `src/notifications/channel.rs`, `src/notifications/queue.rs`, `src/notifications/service.rs`, `src/session/error.rs`.

8. **Prompt & Tool Contract Hardening**
   - [ ] Update system prompt to be robust, concise, toolcalling/action-oriented, and security-hardened for PicoBot as an execution assistant (not only an answer bot). Files: `src/config.rs`, `picobot.example.toml`, `README.md`.
   - [ ] Review and tighten tool descriptions/parameter contracts to reduce ambiguity and unsafe model behavior. Files: `src/tools/*.rs`, `src/tools/traits.rs`.

9. **Security Documentation & Config Guidance**
   - [ ] Security-focused documentation updates and config examples for new authz/network constraints. Files: `README.md`, `picobot.example.toml`, `AGENTS.md`.

### Phase 5: Execution Isolation, Advanced Channels & Productization (Future)

**Goal**: Introduce isolated execution and higher-complexity channels/tools after Phase 4 boundary hardening is complete.

#### Tasks

1. **Shell Governance (Sentinel/HITL)**
   - [ ] Add command classifier (pattern-based, safe/risky/deny). Approach: classify command + args pre-execution and attach decision metadata to audit logs. Files: `src/tools/shell.rs`, new `src/tools/shell_policy.rs`.
   - [ ] Add approval policy to shell permissions config. Approach: configurable deny/risky/allow lists and channel-specific overrides. Files: `src/config.rs`, `picobot.example.toml`.
   - [ ] Implement sync approval for channels that support user prompts. Approach: use prompter path for interactive channels and policy-based behavior for non-interactive channels. Files: `src/kernel/core.rs`, `src/channels/repl.rs`, `src/channels/api.rs`, `src/channels/whatsapp.rs`.
   - [ ] Add approval timeout and fallback behavior. Approach: default-deny on timeout for risky commands, configurable per channel. Files: `src/kernel/core.rs`, `src/channels/permissions.rs`, `src/config.rs`.

2. **Isolated Runtime for High-Risk Tools**
   - [ ] Containerized execution for shell tool (ephemeral containers). Approach: execute shell commands through a runner abstraction backed by OCI containers instead of direct host `tokio::process::Command`. Files: `src/tools/shell.rs`, new `src/tools/shell_runner.rs`, `src/config.rs`.
   - [ ] Resource limits and timeouts for tool execution. Approach: enforce per-tool timeout/memory/output limits in Kernel/tool adapters and return typed timeout/limit errors. Files: `src/kernel/core.rs`, `src/tools/shell.rs`, `src/tools/http.rs`, `src/tools/multimodal_looker.rs`, `src/config.rs`.

3. **Streaming Channel Expansion**
   - [ ] WebSocket server for real-time streaming tokens. Files: new `src/channels/websocket.rs`, `src/main.rs`.

4. **Remote Browser Tool**
   - [ ] Containerized headless Chrome/Chromium. Files: new `src/tools/browser/*.rs`, container/runtime configs.
   - [ ] WebSocket control interface for browser actions. Files: `src/channels/websocket.rs`, `src/tools/browser/*.rs`.
   - [ ] Screenshot and DOM extraction support. Files: `src/tools/browser/*.rs`.

5. **Web Search Tool**
    - [ ] Evaluate options (Google/Exa/Brave vs browser-tool-backed search) and define a narrow safe first implementation. Files: `src/tools/search.rs` (new), `src/config.rs`, `README.md`.

6. **Multi modal tool**
   - [x] A tool to understand documents/images etc and do actions as prescribed by the user. Should be able to configure model to be used

7. **Skill System to add new skills via skill files**
    - [ ] A skill system like openclaw but should be very secure. Any code executions should likely be sandboxed to avoid impacting picobot itself or anything on the server its running on. Files: new `src/skills/*`, `src/tools/*`, `src/config.rs`.
    - [ ] Evaluate whether we need a separate code execution tool or it should be part of this itself. Files: design doc + `README.md`.
    - [ ] Evaluate persistent vs ephemeral skill system. Files: `src/skills/*`, storage schema in `src/session/db.rs` or dedicated store.

8. **Deployment**
    - [ ] Dockerize. Files: `Dockerfile`, compose/dev scripts.
    - [ ] Github CI for mac/linux/windows/docker. Files: `.github/workflows/*`.
    - [ ] Publish release workflow. Files: `.github/workflows/*`, release docs.

## Key Design Decisions

### 1. KernelBackedTool Pattern

Tools should register via `ToolDyn` to support dynamic names while delegating to Kernel:

```rust
pub struct KernelBackedTool {
    spec: ToolSpec,
    kernel: Arc<Kernel>,
}

#[async_trait]
impl ToolDyn for KernelBackedTool {
    fn name(&self) -> String {
        self.spec.name.clone()
    }

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: self.spec.name.clone(),
            description: self.spec.description.clone(),
            parameters: self.spec.schema.clone(),
        }
    }

    async fn call(&self, args: serde_json::Value) -> Result<serde_json::Value, ToolError> {
        self.kernel.invoke_tool(&self.spec.name, args).await
    }
}
```

### 2. ZAI GLM via OpenAI Client

```rust
fn create_zai_client() -> openai::Client {
    openai::Client::from_url("https://open.bigmodel.cn/api/paas/v4/")
        .with_api_key(std::env::var("ZAI_API_KEY").unwrap())
}
```

### 3. Agent Builder

```rust
pub async fn build_agent<M: CompletionModel>(
    client: &M::Client,
    model: &str,
    system_prompt: &str,
    tools: &ToolRegistry,
    kernel: Arc<Kernel>,
) -> Agent<M> {
    let mut builder = client.agent(model).preamble(system_prompt);
    
    for spec in tools.specs() {
        let wrapped = KernelBackedTool::new(spec.clone(), kernel.clone());
        builder = builder.tool_boxed(Box::new(wrapped));
    }
    
    builder.build()
}
```

## File Structure

```
src/
├── main.rs              # Entry point, Server/Channel dispatch
├── config.rs            # Configuration parsing
├── kernel/
│   ├── mod.rs
│   ├── kernel.rs        # Core kernel with permission enforcement
│   ├── permissions.rs   # CapabilitySet, Permission, Scope
│   └── session.rs       # Session context (user/session IDs)
├── providers/
│   ├── mod.rs
│   └── factory.rs       # Provider client factory
├── tools/
│   ├── mod.rs
│   ├── traits.rs        # ToolSpec, ToolExecutor
│   ├── rig_wrapper.rs   # KernelBackedTool
│   ├── registry.rs      # Tool collection
│   ├── filesystem.rs    # File operations
│   ├── shell.rs         # Command execution
│   └── http.rs          # HTTP fetch
└── channels/
    ├── mod.rs
    ├── api.rs           # REST API endpoints
    ├── websocket.rs     # WebSocket streaming
    ├── whatsapp.rs      # WhatsApp adapter
    └── repl.rs          # Development REPL
```

## Dependencies

```toml
[dependencies]
# Core
rig-core = "0.9"
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"
futures = "0.3"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# Validation
jsonschema = "0.26"

# Storage
rusqlite = { version = "0.33", features = ["chrono"] }

# HTTP (for tools)
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls"] }

# Server
axum = { version = "0.8", features = ["ws"] }
tower-http = { version = "0.6", features = ["cors", "trace"] }

# Utilities
anyhow = "1"
thiserror = "2"
glob = "0.3"
chrono = { version = "0.4", features = ["serde"] }
dirs = "6"
uuid = { version = "1", features = ["v4"] }
```

## Success Criteria

1. **Reliability**: No tool call duplications or message corruption
2. **Streaming**: Tokens appear as they're generated
3. **Permissions**: All tool calls checked via Kernel
4. **Provider switching**: Change model via config, same behavior
5. **Code reduction**: ~1500 fewer lines than reference

## Risks & Mitigations

| Risk | Mitigation |
|------|------------|
| Rig-core API changes | Pin version, test before upgrade |
| Permission bypass | Kernel-only execution, thorough testing |
| Missing features | Check reference before implementing |
| Performance regression | Benchmark streaming paths |
