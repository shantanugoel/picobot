# PicoBot Agent Guide

This file provides guidance for AI agents working on the PicoBot rewrite. Read `PLAN.md` for the full implementation plan.

## Project Status

**This is a ground-up rewrite.** The previous implementation is in `reference/` for guidance only.

### Guidelines
- While porting any changes from reference, remember that we should not port them as is. Many of the hacks/workarounds etc might be taken care of by rig-core so we need to think through thoroughly and then write the code.
- Whenever you make a change, make sure to check for warnings/errors/tests by running `cargo check`, `cargo clippy`, `cargo test`
- Always use latest versions of any rust crates
- When making any plans, consult with oracle subagent to get its recommendation about your plan and see if they make sense and incorporate in yours.


## Quick Reference

## Core Principles

### 1. Rig-Core is the Foundation

We use `rig-core` for:
- AI provider abstraction (OpenAI, Anthropic, Gemini, OpenRouter)
- Tool calling via the `Tool` trait
- Agent orchestration via `multi_turn()`
- Streaming via `stream_prompt()`

**DO NOT** implement custom agent loops or message sanitization - rig-core handles this.

### 2. Kernel is the Single Enforcement Point

All tool execution MUST go through the Kernel:

```rust
// CORRECT: Tool delegates to Kernel
impl Tool for KernelBackedTool {
    async fn call(&self, args: Value) -> Result<Value, Error> {
        self.kernel.invoke_tool(&self.name, args).await
    }
}

// WRONG: Tool executes directly
impl Tool for DirectTool {
    async fn call(&self, args: Value) -> Result<Value, Error> {
        std::fs::read_to_string(args["path"].as_str().unwrap()) // NO!
    }
}
```

### 3. Permissions are Allowlist-Based

- No implicit grants
- Tools declare required permissions, Kernel checks them
- CapabilitySet uses glob matching for paths
- Session grants can be temporary (once) or session-scoped

## Architecture Overview

```
User Input
    │
    ▼
Rig Agent (multi_turn)
    │
    ├─── Provider (OpenAI/Anthropic/Gemini)
    │
    └─── Tools (KernelBackedTool)
              │
              ▼
         Kernel.invoke_tool()
              │
              ├── Check permissions (CapabilitySet)
              ├── Request permission if needed (callback)
              ├── Execute tool
              └── Return wrapped output
```

## Key Files to Create

| File | Purpose | Reference |
|------|---------|-----------|
| `src/kernel/permissions.rs` | Permission types, CapabilitySet | Port from `reference/src/kernel/permissions.rs` |
| `src/kernel/kernel.rs` | Enforcement, tool invocation | Adapt `reference/src/kernel/agent.rs` |
| `src/tools/rig_wrapper.rs` | KernelBackedTool | New implementation |
| `src/tools/filesystem.rs` | File operations | Port from `reference/src/tools/filesystem.rs` |
| `src/providers/factory.rs` | Create rig-core clients | New implementation |

## Key Files to NOT Port

| Reference File | Reason |
|----------------|--------|
| `reference/src/kernel/agent_loop.rs` | Replaced by rig-core agent |
| `reference/src/models/genai_adapter.rs` | Replaced by rig-core providers |
| `reference/src/models/types.rs` | Use rig-core types |

## Invariants (Do Not Break)

1. **All tool input must be schema-validated** before execution
2. **All tool calls must pass through Kernel** for permission check
3. **Tool output is untrusted data** - wrap before feeding to model
4. **Capability checks are allowlist-based** - no implicit grants

## Code Patterns

### Creating a Provider Client

```rust
use rig::providers::{openai, anthropic};

// OpenAI
let openai = openai::Client::from_env();

// Anthropic
let anthropic = anthropic::Client::from_env();

// Custom base URL (ZAI GLM, OpenRouter, etc.)
let zai = openai::Client::from_url("https://open.bigmodel.cn/api/paas/v4/")
    .with_api_key(std::env::var("ZAI_API_KEY")?);
```

### Building an Agent with Tools

```rust
let agent = client
    .agent("gpt-4o")
    .preamble("You are a helpful assistant.")
    .tool(filesystem_tool)
    .tool(shell_tool)
    .build();
```

### Running with Multi-Turn Tool Calling

```rust
let response = agent
    .prompt("List files in /tmp and show me the largest one")
    .multi_turn(10)  // Max 10 tool iterations
    .await?;
```

### KernelBackedTool Pattern

```rust
pub struct KernelBackedTool {
    name: String,
    description: String,
    schema: Value,
    kernel: Arc<Kernel>,
}

impl Tool for KernelBackedTool {
    const NAME: &'static str = "";  // Dynamic
    type Args = serde_json::Value;
    type Output = serde_json::Value;
    type Error = ToolError;

    async fn definition(&self, _prompt: String) -> ToolDefinition {
        ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.schema.clone(),
        }
    }

    async fn call(&self, args: Self::Args) -> Result<Self::Output, Self::Error> {
        self.kernel.invoke_tool(&self.name, args).await
    }
}
```

## Testing Expectations

- Unit tests for permission logic in `src/kernel/permissions.rs`
- Unit tests for tool schema validation
- Integration tests for tool execution via Kernel
- Keep tests close to the code they test

## Common Mistakes to Avoid

1. **Don't bypass Kernel** - All tool execution goes through `Kernel::invoke_tool`
2. **Don't implement custom message sequencing** - Rig-core handles this
3. **Don't collect streaming into Vec** - Use async streams
4. **Don't use genai crate** - We're replacing it with rig-core
5. **Don't copy agent_loop.rs** - It's replaced by rig-core's agent

## Debugging Tips

- Check `reference/` for how things worked before
- Use `RUST_LOG=debug` for verbose logging
- Rig-core has tracing built-in, enable with `RUST_LOG=rig=debug`

## Dependencies Note

Always use latest versions of crates. Check crates.io for current versions before adding dependencies.

## Questions?

If something is unclear:
1. Check `PLAN.md` for the implementation plan
2. Check `reference/` for how it was done before
3. Check rig-core documentation at https://docs.rs/rig-core
