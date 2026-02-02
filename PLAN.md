# PicoBot - Architecture & Implementation Plan

A secure, extensible AI agent in Rust with modular model support and capability-based security.
Inspired by OpenClaw but intentionally simpler and more security-focused.

## Design Principles

1. **Security-first**: Capability-based permissions enforced at the kernel level
2. **Simple core, extensible edges**: Small kernel, pluggable tools/models
3. **Start minimal**: MVP with few tools, expand organically
4. **No over-engineering**: Static registration before dynamic plugins

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

---

## Core Traits

### Model Trait
```rust
#[async_trait]
pub trait Model: Send + Sync {
    fn info(&self) -> ModelInfo;
    async fn complete(&self, req: ModelRequest) -> Result<ModelResponse>;
    async fn stream(&self, req: ModelRequest) -> Result<BoxStream<'static, ModelEvent>>;
}

pub struct ModelRequest {
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
}

pub enum ModelEvent {
    Token(String),
    ToolCall(ToolInvocation),
    Done(ModelResponse),
}
```

### Tool Trait
```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn schema(&self) -> ToolSchema;  // JSON Schema for input validation
    fn required_permissions(&self) -> Vec<Permission>;
    async fn execute(&self, ctx: &ToolContext, input: Value) -> Result<ToolOutput>;
}
```

### Permission System
```rust
pub enum Permission {
    FileRead { path: PathPattern },
    FileWrite { path: PathPattern },
    NetAccess { domain: DomainPattern },
    ShellExec { allowed_commands: Option<Vec<String>> },
}

pub struct CapabilitySet {
    permissions: HashSet<Permission>,
}

impl CapabilitySet {
    pub fn allows(&self, required: &Permission) -> bool;
    pub fn allows_all(&self, required: &[Permission]) -> bool;
}
```

### Model Router
```rust
pub trait ModelRouter: Send + Sync {
    fn select_model(&self, request: &ModelRequest, task_hint: Option<&str>) -> ModelId;
}
```

---

## Project Structure

```
picobot/
├── Cargo.toml
├── config.example.toml
├── AGENTS.md
├── PLAN.md
├── README.md
├── src/
│   ├── main.rs                    # CLI entrypoint
│   ├── lib.rs                     # Library root
│   │
│   ├── kernel/
│   │   ├── mod.rs
│   │   ├── agent.rs               # Main agent loop/orchestrator
│   │   ├── permissions.rs         # Permission types & checking
│   │   └── context.rs             # Execution context
│   │
│   ├── models/
│   │   ├── mod.rs
│   │   ├── traits.rs              # Model trait definitions
│   │   ├── openai_compat.rs       # OpenAI-compatible implementation
│   │   ├── router.rs              # Model selection logic
│   │   └── types.rs               # Message, ToolCall, etc.
│   │
│   ├── tools/
│   │   ├── mod.rs
│   │   ├── traits.rs              # Tool trait definitions
│   │   ├── registry.rs            # Tool registration
│   │   ├── schema.rs              # JSON Schema validation
│   │   ├── shell.rs               # Shell command execution
│   │   ├── filesystem.rs          # File read/write
│   │   └── http.rs                # HTTP fetch
│   │
│   ├── cli/
│   │   ├── mod.rs
│   │   └── repl.rs                # Interactive REPL
│   │
│   └── config.rs                  # Configuration parsing
│
└── tests/
    ├── permissions_test.rs
    ├── tools_test.rs
    └── integration_test.rs
```

---

## Configuration

```toml
# config.toml

[agent]
name = "PicoBot"
system_prompt = """
You are PicoBot, a helpful AI assistant with access to tools.
Always explain what you're about to do before using a tool.
"""

# Model configurations - OpenAI-compatible endpoints
[[models]]
id = "default"
provider = "openai"
model = "gpt-4o"
api_key_env = "OPENAI_API_KEY"

[[models]]
id = "local"
provider = "ollama"
model = "llama3.2"
base_url = "http://localhost:11434/v1"

[[models]]
id = "anthropic"
provider = "openrouter"
model = "anthropic/claude-sonnet-4"
base_url = "https://openrouter.ai/api/v1"
api_key_env = "OPENROUTER_API_KEY"

# Model routing
[routing]
default = "default"

# Strict permissions - explicit grants only
[permissions.filesystem]
read_paths = ["~/Documents/**", "~/Projects/**", "/tmp/**"]
write_paths = ["/tmp/picobot/**"]

[permissions.network]
allowed_domains = ["api.github.com", "httpbin.org"]

[permissions.shell]
allowed_commands = ["ls", "cat", "head", "tail", "wc", "grep", "find", "git"]
working_directory = "~/Projects"

[logging]
level = "info"
audit_file = "~/.picobot/audit.log"
```

---

## Dependencies

```toml
[dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }
async-trait = "0.1"

# OpenAI-compatible API
async-openai = "0.27"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# HTTP client
reqwest = { version = "0.12", features = ["json", "stream"] }

# Streaming
futures = "0.3"
async-stream = "0.3"

# CLI
clap = { version = "4", features = ["derive"] }
rustyline = "15"
colored = "3"

# Validation
jsonschema = "0.26"

# Error handling
thiserror = "2"
anyhow = "1"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Path patterns
glob = "0.3"
```

---

## Implementation Phases

### Phase 1: Foundation (Days 1-3)
- [x] Project setup with Cargo.toml
- [x] Config parsing (TOML)
- [x] Permission types and CapabilitySet
- [x] Error types with thiserror
- [x] Model trait definition
- [x] Tool trait definition

### Phase 2: Model Layer (Days 4-5)
- [x] OpenAI-compatible model implementation
- [x] Message types (User, Assistant, Tool, System)
- [x] Tool call handling (function calling)
- [x] Streaming support
- [x] Simple router (single model)

### Phase 3: Tools (Days 6-8)
- [x] Tool registry
- [x] Schema validation with jsonschema
- [x] Shell tool (with command allowlist)
- [x] Filesystem tool (read/write with path validation)
- [x] HTTP fetch tool (with domain allowlist)

### Phase 4: Kernel (Days 9-11)
- [x] Agent orchestration loop
- [x] Permission checking integration
- [x] Tool execution flow
- [x] Conversation state management
- [x] Error handling and recovery

### Phase 5: CLI (Days 12-14)
- [ ] TUI with Ratatui
- [ ] Command history
- [ ] Colored output
- [ ] Special commands (/quit, /clear, /permissions)
- [ ] Streaming output display

### Phase 6: Polish
- [ ] Comprehensive tests
- [ ] Documentation
- [x] Example configs
- [ ] README with usage instructions

---

## Security Model

### Capability-Based Permissions

1. **Every tool declares required permissions** at compile time
2. **Sessions have a CapabilitySet** granted via config
3. **Kernel validates before every tool call** - single enforcement point
4. **All sensitive actions are logged** for audit

### Permission Check Flow
```rust
impl Kernel {
    async fn invoke_tool(&self, tool: &dyn Tool, input: Value) -> Result<Value> {
        // 1. Check permissions
        let required = tool.required_permissions();
        if !self.session.capabilities.allows_all(&required) {
            self.audit.record_denied(tool.name(), &required);
            return Err(ToolError::PermissionDenied(tool.name().into()));
        }
        
        // 2. Validate input against schema
        tool.schema().validate(&input)?;
        
        // 3. Execute
        let result = tool.execute(&self.context, input).await;
        
        // 4. Audit
        self.audit.record_executed(tool.name(), result.is_ok());
        
        result
    }
}
```

### Sandboxing (Future)
- Process isolation for shell commands
- WASI sandbox for untrusted extensions
- Resource limits (CPU, memory, time)

---

## Agent Loop

```rust
async fn run_agent_loop(&mut self) -> Result<()> {
    loop {
        // 1. Get user input
        let user_message = self.cli.read_input()?;
        self.conversation.push(Message::user(user_message));
        
        // 2. Call model with tools
        let response = self.model.complete(ModelRequest {
            messages: self.conversation.clone(),
            tools: self.registry.tool_specs(),
            ..Default::default()
        }).await?;
        
        // 3. Handle response
        match response {
            ModelResponse::Text(text) => {
                self.cli.print_assistant(&text);
                self.conversation.push(Message::assistant(text));
            }
            ModelResponse::ToolCalls(calls) => {
                for call in calls {
                    // Execute tool (with permission check)
                    let result = self.invoke_tool(&call).await;
                    self.conversation.push(Message::tool_result(call.id, result));
                }
                // Continue loop to get model's response to tool results
                continue;
            }
        }
    }
}
```

---

## Future Enhancements

- **HTTP API**: REST/WebSocket for remote access
- **Communication adapters**: Telegram, Discord, Slack
- **Persistent memory**: SQLite for conversation history
- **Multi-model routing**: Task-based model selection
- **Dynamic tools**: Runtime tool loading (WASM plugins)
- **Heartbeats**: Proactive scheduled tasks
