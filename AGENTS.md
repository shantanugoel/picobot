# AGENTS.md - PicoBot Coding Guidelines

## Project Context

PicoBot is a secure AI agent in Rust. Read `PLAN.md` for full architecture.

**Critical invariants:**
- All tool invocations MUST go through the kernel's permission guard
- Never bypass `CapabilitySet.allows()` checks
- Tools declare permissions; kernel enforces them
- Always use the latest versions of any crates or dependencies
- After making any changes, always run `cargo check` and `cargo clippy` to make sure there are no warnings or errors

## Design Principles

1. **Security-first**: Capability-based permissions enforced at the kernel level
2. **Simple core, extensible edges**: Small kernel, pluggable tools/models
4. **No over-engineering**: 

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                      CLI REPL Interface                      │
│                (Future: HTTP API, Telegram, etc.)            │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                         CORE KERNEL                          │
│  ┌─────────────┐  ┌──────────────┐  ┌───────────────────┐   │
│  │ Agent Loop  │  │  Permission  │  │   Model Router    │   │
│  │(Orchestrator)│  │    Guard     │  │   (Task-based)    │   │
│  └─────────────┘  └──────────────┘  └───────────────────┘   │
└─────────────────────────────────────────────────────────────┘
         │                   │                    │
         ▼                   ▼                    ▼
┌─────────────────┐  ┌──────────────┐  ┌──────────────────────┐
│   Tool Registry │  │   Audit Log  │  │    Model Registry    │
│   (Capabilities)│  │   (tracing)  │  │  (OpenAI-compat)     │
└─────────────────┘  └──────────────┘  └──────────────────────┘
         │                                       │
         ▼                                       ▼
┌─────────────────┐                   ┌──────────────────────┐
│ Built-in Tools  │                   │  Providers via       │
│ - Shell         │                   │  async-openai:       │
│ - FileSystem    │                   │  - OpenAI            │
│ - HTTP/Fetch    │                   │  - Ollama            │
└─────────────────┘                   │  - OpenRouter        │
                                      │  - Any compatible    │
                                      └──────────────────────┘
```

## Security Model

### Capability-Based Permissions

1. **Every tool declares required permissions** at compile time
2. **Sessions have a CapabilitySet** granted via config
3. **Kernel validates before every tool call** - single enforcement point
4. **All sensitive actions are logged** for audit

## Code Organization

```
src/
├── kernel/          # Orchestration, permissions - THE security boundary
├── models/          # Model trait + OpenAI-compatible impl
├── tools/           # Tool trait + built-in tools (shell, fs, http)
├── cli/             # REPL interface
└── config.rs        # TOML config parsing
```

## Import Order

```rust
use std::...;           // 1. std

use async_trait::...;   // 2. external crates
use serde::...;

use crate::kernel::...; // 3. local crate
```

## Error Handling

- **Library code**: Use `thiserror`, propagate with `?`
- **main.rs/CLI**: Use `anyhow` for convenience
- **Never** use `.unwrap()` in production paths
- `.expect()` only for impossible states with clear message

```rust
// In lib code
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    #[error("Permission denied for tool '{tool}': requires {required:?}")]
    PermissionDenied { tool: String, required: Vec<Permission> },
}
```

## Async Patterns

- All traits with async methods use `#[async_trait]`
- Trait objects must be `Send + Sync`
- Shared state: `Arc<RwLock<T>>`

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput>;
}
```

## Security Rules

1. **Permission checks happen in ONE place**: `kernel/agent.rs::invoke_tool()`
2. **Tools never check their own permissions** - they declare, kernel enforces
3. **Validate all inputs** against JSON schema before execution
4. **Log all sensitive operations** via the audit system
5. **Allowlists over denylists** - explicit grants, not blocks

## Adding a New Tool

1. Create `src/tools/my_tool.rs`
2. Implement `Tool` trait with:
   - `name()` - unique identifier
   - `description()` - for LLM context
   - `schema()` - JSON Schema for input validation
   - `required_permissions()` - what capabilities it needs
   - `execute()` - the actual logic
3. Register in `src/tools/mod.rs`

```rust
pub struct MyTool;

#[async_trait]
impl Tool for MyTool {
    fn name(&self) -> &'static str { "my_tool" }
    
    fn required_permissions(&self) -> Vec<Permission> {
        vec![Permission::NetAccess { domain: "api.example.com".into() }]
    }
    
    async fn execute(&self, _ctx: &ToolContext, input: Value) -> Result<ToolOutput> {
        // Tool logic here - permission already verified by kernel
    }
}
```

## Adding a New Model Provider

1. Create `src/models/my_provider.rs`
2. Implement `Model` trait
3. Must handle: streaming, tool calls, error recovery
4. Register in model registry

## Testing

- Unit tests: `#[cfg(test)] mod tests` in same file
- Integration tests: `tests/` directory
- Test permission denials explicitly
- Use `#[tokio::test]` for async tests

```rust
#[tokio::test]
async fn test_tool_without_permission_is_denied() {
    let kernel = Kernel::new(CapabilitySet::empty());
    let result = kernel.invoke_tool(&ShellTool, json!({"cmd": "ls"})).await;
    assert!(matches!(result, Err(ToolError::PermissionDenied { .. })));
}
```

## Common Mistakes

| Mistake | Fix |
|---------|-----|
| Checking permissions inside tool | Move to kernel, tool only declares |
| Using `.unwrap()` on user input | Use `?` with proper error type |
| Forgetting `Send + Sync` on trait | Add bounds: `trait Foo: Send + Sync` |
| Hardcoding paths/URLs | Use config, validate against permissions |
| Skipping schema validation | Always validate before `execute()` |

## Key Types to Know

```rust
// Core permission type - see kernel/permissions.rs
pub enum Permission {
    FileRead { path: PathPattern },
    FileWrite { path: PathPattern },
    NetAccess { domain: DomainPattern },
    ShellExec { allowed_commands: Option<Vec<String>> },
}

// Tool execution context - see kernel/context.rs
pub struct ToolContext {
    pub working_dir: PathBuf,
    pub capabilities: Arc<CapabilitySet>,
    pub audit: Arc<AuditLog>,
}

// Model request - see models/types.rs
pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u32>,
}
```

## Config Reference

See `config.example.toml`. Key sections:
- `[agent]` - name, system prompt
- `[[models]]` - model provider configs
- `[permissions.*]` - capability grants (filesystem, network, shell)
