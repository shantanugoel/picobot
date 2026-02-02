# PicoBot - Implementation Plan

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
- [x] TUI with Ratatui
- [x] Command history
- [x] Colored output
- [x] Special commands (/quit, /clear, /permissions)
- [x] Streaming output display

### Phase 6: Polish
- [ ] Comprehensive tests
- [ ] Documentation
- [x] Example configs
- [ ] README with usage instructions

---

### Sandboxing (Future)
- Process isolation for shell commands
- WASI sandbox for untrusted extensions
- Resource limits (CPU, memory, time)

## Future Enhancements

- **HTTP API**: REST/WebSocket for remote access
- **Communication adapters**: Telegram, Discord, Slack, WhatsApp
- **Persistent memory**: SQLite for conversation history
- **Multi-model routing**: Task-based model selection
- **Dynamic tools**: Runtime tool loading (WASM plugins)
- **Heartbeats**: Proactive scheduled tasks
