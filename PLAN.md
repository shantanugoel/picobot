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
   - [ ] Update README.md
   - [ ] Configuration reference

#### Milestone: Feature parity with reference implementation.

### Phase 4: Security & Polish (Week 7)

**Goal**: Harden execution paths and tighten production readiness.

#### Tasks

1. **Execution Isolation**
   - [ ] Containerized execution for shell tool (ephemeral containers)
   - [ ] Resource limits and timeouts for tool execution

2. **Sentinel (HITL) for Shell Commands**
   - [ ] Add command classifier (pattern-based, safe/risky/deny)
   - [ ] Add approval policy to shell permissions config
   - [ ] Implement sync approval for REPL channel
   - [ ] Implement async approval via WebSocket channel
   - [ ] Add approval timeout and fallback behavior

3. **Filesystem Hardening**
   - [ ] Canonicalize/normalize all paths before checks
   - [ ] Re-check canonical paths at execution time for writes
   - [ ] Check if incoming media/documents etc from different users e.g. over whatsapp (or any other channel) have leakage potential to other users via filesystem or shell or other tools

4. **Audit & Diagnostics**
   - [ ] Structured audit logs for tool usage
   - [ ] Log approval decisions with context
   - [ ] Security-focused documentation updates

### Phase 5: Server Channels & Advanced Tools (Future)

**Goal**: Add REST/WS channels and higher-complexity tools behind hardened boundaries.

#### Tasks

1. **Server Core** (`src/channels/`)
   - [ ] Implement Axum-based REST API
   - [ ] WebSocket server for real-time streaming tokens

2. **Remote Browser Tool**
   - [ ] Containerized headless Chrome/Chromium
   - [ ] WebSocket control interface for browser actions
   - [ ] Screenshot and DOM extraction support

3. **Multi modal tool**
   - [ ] A tool to understand documents/images etc and do actions as prescribed by the user. Should be able to configure model to be used

4. **Sandboxed code executor tool**
    - [ ] Should be able to leverage a configured model to write code and then execute it, but only in a sandboxed environment so it does not impact itself or anything else on the server its running on.

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
