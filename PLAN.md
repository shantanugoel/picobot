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
│                         User / TUI / API                          │
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
│                  KernelBackedTool (impl rig::Tool)                │
│  - Holds Arc<Kernel> reference                                    │
│  - call() delegates to Kernel.invoke_tool_with_grants()           │
│  - Preserves CapabilitySet, session grants, permission prompts    │
└───────────────────────────────────────┬───────────────────────────┘
                                        │
                                        ▼
┌───────────────────────────────────────────────────────────────────┐
│                         Kernel                                    │
│  - Permission enforcement (CapabilitySet)                         │
│  - Glob-based path matching                                       │
│  - Session grants management                                      │
│  - Permission prompt callbacks (for TUI)                          │
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
| `reference/src/cli/tui.rs` | TUI implementation | Port with streaming updates |
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
   - [ ] Create `Cargo.toml` with rig-core and essential dependencies
   - [ ] Create `src/main.rs` with basic structure
   - [ ] Create `rust-toolchain.toml` (use stable, not nightly if possible)

2. **Kernel Core** (`src/kernel/`)
   - [ ] Port `permissions.rs` (CapabilitySet, Permission, Scope types)
   - [ ] Create `kernel.rs` with permission checking and tool invocation
   - [ ] Create `session.rs` for session grants management

3. **Tool Infrastructure** (`src/tools/`)
   - [ ] Create `traits.rs` with ToolSpec definition
   - [ ] Create `rig_wrapper.rs` with KernelBackedTool implementation
   - [ ] Port `filesystem.rs` as first tool (read/write/list)
   - [ ] Create `registry.rs` for tool collection

4. **Provider Setup** (`src/providers/`)
   - [ ] Create `factory.rs` for building rig-core clients
   - [ ] Support OpenAI provider with configurable base_url
   - [ ] Create agent builder function

5. **Basic CLI** (`src/cli/`)
   - [ ] Create minimal REPL loop (no TUI yet)
   - [ ] Wire up agent execution with tool calls
   - [ ] Add permission prompt via stdin

6. **Configuration** (`src/config.rs`)
   - [ ] Port minimal config (model, permissions, data dir)

#### Milestone: Run `cargo run`, chat with agent, execute filesystem tool with permission prompt.

### Phase 2: Multi-Provider & Tools (Week 3-4)

**Goal**: Full provider support and complete tool set.

#### Tasks

1. **Additional Providers**
   - [ ] Add Anthropic client
   - [ ] Add Gemini client
   - [ ] Add OpenRouter client
   - [ ] Add ZAI GLM via OpenAI client with custom base_url
   - [ ] Create provider routing based on config

2. **Complete Tool Set**
   - [ ] Port `shell.rs` (command execution)
   - [ ] Port `http.rs` (HTTP fetch)
   - [ ] Port `schedule.rs` (scheduling)

3. **Streaming Support**
   - [ ] Implement streaming output to CLI
   - [ ] Handle tool call events during stream

4. **Session & Memory**
   - [ ] Port SQLite session storage
   - [ ] Implement chat history management
   - [ ] Port memory retrieval for context

5. **Configuration Expansion**
   - [ ] Add multi-model configuration
   - [ ] Add channel permissions (pre_authorized, max_allowed)

#### Milestone: Switch between providers, all tools working, streaming output.

### Phase 3: TUI & Polish (Week 5-6)

**Goal**: Full-featured TUI and production readiness.

#### Tasks

1. **TUI Implementation** (`src/cli/tui.rs`)
   - [ ] Port Ratatui-based TUI
   - [ ] Streaming token display
   - [ ] Permission prompt UI
   - [ ] Command handling (/help, /clear, /new, etc.)

2. **Server Mode** (optional, lower priority)
   - [ ] Port API server (if needed)
   - [ ] WebSocket support
   - [ ] WhatsApp adapter (if needed)

3. **Robustness**
   - [ ] Error handling and recovery
   - [ ] Logging and audit trail
   - [ ] Configuration validation

4. **Testing**
   - [ ] Unit tests for Kernel
   - [ ] Unit tests for permissions
   - [ ] Integration tests for tool execution

5. **Documentation**
   - [ ] Update README.md
   - [ ] Configuration reference

#### Milestone: Feature parity with reference implementation.

## Key Design Decisions

### 1. KernelBackedTool Pattern

Tools implement rig-core's `Tool` trait but delegate execution to Kernel:

```rust
pub struct KernelBackedTool {
    spec: ToolSpec,
    kernel: Arc<Kernel>,
    permission_callback: Arc<dyn PermissionCallback>,
}

#[async_trait]
impl Tool for KernelBackedTool {
    const NAME: &'static str = ""; // Dynamic
    type Args = serde_json::Value;
    type Output = serde_json::Value;
    type Error = ToolError;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: self.spec.name.clone(),
            description: self.spec.description.clone(),
            parameters: self.spec.schema.clone(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
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

### 3. Permission Prompt Flow

```rust
pub trait PermissionCallback: Send + Sync {
    async fn request_permission(
        &self,
        tool: &str,
        permissions: &[Permission],
    ) -> PermissionDecision;
}

pub enum PermissionDecision {
    Allow,
    AllowSession,
    Deny,
}
```

### 4. Agent Builder

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
        builder = builder.tool(wrapped);
    }
    
    builder.build()
}
```

## File Structure

```
src/
├── main.rs              # Entry point, CLI/TUI dispatch
├── config.rs            # Configuration parsing
├── kernel/
│   ├── mod.rs
│   ├── kernel.rs        # Core kernel with permission enforcement
│   ├── permissions.rs   # CapabilitySet, Permission, Scope
│   └── session.rs       # Session grants, memory
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
└── cli/
    ├── mod.rs
    ├── repl.rs          # Simple REPL (Phase 1)
    └── tui.rs           # Ratatui TUI (Phase 3)
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

# CLI
crossterm = "0.28"
ratatui = "0.29"

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

## Getting Started

```bash
# Start fresh
cargo init

# Add dependencies to Cargo.toml

# Run tests from reference for guidance
cd reference && cargo test

# Begin with Phase 1, Task 1
```
