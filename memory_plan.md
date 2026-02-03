# Phase 2: Persistent Memory Plan

This document captures the detailed plan for Phase 2: Persistent Memory. It reflects all decisions and updates from the discussion, including storage location, retention defaults, explicit user memories, purge behavior, and context bloat controls.

## Decisions

- Data directory: `data/` (configurable via `data.dir`)
- Default retention: 90 days (configurable)
- User-scoped memory: enabled, explicit "remember" flow, list/delete support
- Purge confirmation: y/n (TUI/API only)
- Audit logging: regular logs (no separate audit file)
- Context bloat: enforced with token budget, message limits, compact memories, summaries

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────┐
│                        PicoBot Memory System                             │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  ┌─────────────┐     ┌─────────────────────────────────────────────┐   │
│  │   Channel   │     │              Kernel                         │   │
│  │  Adapters   │────▶│  ┌─────────────┐  ┌──────────────────────┐  │   │
│  │ (WA/WS/TUI) │     │  │ ToolContext │  │ PrivacyController    │  │   │
│  └─────────────┘     │  │ + user_id   │  │ (purge operations)   │  │   │
│        │             │  └─────────────┘  └──────────────────────┘  │   │
│        │             │         │                                    │   │
│        ▼             │         ▼                                    │   │
│  ┌─────────────┐     │  ┌─────────────────────────────────────┐    │   │
│  │   Session   │     │  │           Agent Loop                │    │   │
│  │   Manager   │◀───▶│  │  ┌─────────────────────────────┐   │    │   │
│  │ (Persistent)│     │  │  │ Memory Retriever            │   │    │   │
│  └──────┬──────┘     │  │  │ - User memories (compact)   │   │    │   │
│         │            │  │  │ - Session history (limited) │   │    │   │
│         │            │  │  │ - Summary (if truncated)    │   │    │   │
│         ▼            │  │  │ - Context budget enforced   │   │    │   │
│  ┌─────────────┐     │  │  └─────────────────────────────┘   │    │   │
│  │   SQLite    │     │  └─────────────────────────────────────┘    │   │
│  │  Database   │     │                                              │   │
│  │             │     │  ┌─────────────────────────────────────┐    │   │
│  │ - sessions  │     │  │         Memory Tool                 │    │   │
│  │ - messages  │◀────│  │  save/list/delete user memories     │    │   │
│  │ - memories  │     │  │  (auto-granted for own data)        │    │   │
│  │ - summaries │     │  └─────────────────────────────────────┘    │   │
│  └─────────────┘     │                                              │   │
│                      │                                              │   │
│  ┌─────────────┐     │  ┌─────────────────────────────────────┐    │   │
│  │  Retention  │     │  │       Summarization Task            │    │   │
│  │    Task     │     │  │  (triggers at 8k token threshold)   │    │   │
│  │ (90 day TTL)│     │  └─────────────────────────────────────┘    │   │
│  └─────────────┘     └──────────────────────────────────────────────┘   │
│                                                                         │
└─────────────────────────────────────────────────────────────────────────┘
```

## SQLite Schema

```
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    channel_type TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    permissions_json TEXT NOT NULL,
    created_at TEXT NOT NULL,
    last_active TEXT NOT NULL,
    state_json TEXT NOT NULL,
    summary TEXT
);

CREATE TABLE IF NOT EXISTS messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    message_type TEXT NOT NULL CHECK(message_type IN ('system', 'user', 'assistant', 'tool')),
    content TEXT NOT NULL,
    tool_call_id TEXT,
    created_at TEXT NOT NULL,
    seq_order INTEGER NOT NULL,
    token_estimate INTEGER
);

CREATE TABLE IF NOT EXISTS user_memories (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    content TEXT NOT NULL,
    source_session_id TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    UNIQUE(user_id, key)
);

CREATE TABLE IF NOT EXISTS session_summaries (
    session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    summary TEXT NOT NULL,
    message_count INTEGER NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_session_order ON messages(session_id, seq_order);
CREATE INDEX IF NOT EXISTS idx_messages_created ON messages(created_at);
CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_channel ON sessions(channel_id);
CREATE INDEX IF NOT EXISTS idx_user_memories_user ON user_memories(user_id);
CREATE INDEX IF NOT EXISTS idx_session_summaries_session ON session_summaries(session_id);
```

## User ID Strategy

- WhatsApp: `whatsapp:<phone>@s.whatsapp.net`
- WebSocket: `ws:<api_key_prefix>` or `ws:<session_id>`
- TUI: `tui:local` or `tui:<configured_name>`
- API: `api:<api_key_prefix>`

Implementation detail: `ToolContext` will carry `user_id` and `session_id` (optional) populated by channel adapters.

## Context Bloat Controls

### Context Budget Defaults

- max_tokens: 4000 (memory context)
- max_session_messages: 20
- max_user_memories: 50
- include_summary_on_truncation: true

### Summarization Strategy

- Trigger at ~8000 tokens of session history
- Keep last 10 messages verbatim
- Store summary in `session_summaries`
- Inject summary only when history is truncated

## Config Updates

```
[data]
dir = "data"

[session]
# Derived from data.dir unless explicitly overridden internally
# - conversation db: data/conversations.db
# - sessions json:  data/sessions.json

[session.retention]
max_age_days = 90
cleanup_interval_secs = 3600

[session.memory]
enable_user_memories = true
context_budget_tokens = 4000
max_session_messages = 20
max_user_memories = 50
enable_summarization = true
```

## Permissions

New permission variants:

- `MemoryRead { scope: Session | User | Global }`
- `MemoryWrite { scope: Session | User | Global }`

Auto-granted for user’s own data:

- Session and User scopes are auto-granted
- Global scope still requires explicit grant

## New Components and Files

### Session Layer

- `src/session/db.rs` - SQLite store + worker thread
- `src/session/persistent_manager.rs` - DB-first session manager
- `src/session/retention.rs` - retention task
- `src/session/summarization.rs` - summary generation
- `src/session/migration.rs` - JSON snapshot migration
- `src/session/error.rs` - DB errors

### Kernel Layer

- `src/kernel/memory.rs` - memory retrieval + context budget
- `src/kernel/privacy.rs` - purge operations (kernel-level)
- `src/kernel/context.rs` - add `user_id`, `session_id`

### Tools

- `src/tools/memory.rs` - save/list/delete user memories

### Config and Integration

- `src/config.rs` - new `RetentionConfig`, `MemoryConfig`
- `src/server/state.rs` - initialize persistence, retention, summaries
- `src/cli/tui.rs` - purge commands (y/n confirmation)

## Memory Tool Behavior

### Actions

- `save`: store explicit memory (`key`, `content` required)
- `list`: retrieve all user memories
- `delete`: remove a memory by key

### Memory Key Rules

- Pattern: `^[a-z][a-z0-9_]*$`
- Max length: 64
- Reserved prefixes: `system_`, `internal_`

## Purge Behavior

Purge is a kernel-level operation, not a tool call.

Scopes:

- Session: delete all data for current session
- User: delete all user data
- Older-than: delete messages older than N days

Triggered via TUI/API commands with y/n confirmation. Logged to regular logs.

## Migration Strategy

If `sessions.json` exists under `data/`:

- Load snapshot
- Import sessions/messages to SQLite
- Rename snapshot to `sessions.json.migrated` if successful
- Continue on failure (fresh DB)

## Execution Order

1. Add `rusqlite` dependency
2. Add DB error types
3. Implement SQLite store + schema
4. Add persistent session manager
5. Add migration from JSON snapshots
6. Extend config for DB/retention/memory
7. Extend tool context with user_id/session_id
8. Add memory permissions + auto-grant logic
9. Add memory retrieval + context budget
10. Add privacy controller
11. Add memory tool
12. Inject memory into agent loop
13. Add retention task
14. Add summarization task
15. Wire in server startup + channel adapters
16. Add TUI purge commands
17. Run `cargo check` and `cargo clippy`

## Notes

- DB is the source of truth; in-memory cache is read-through
- WAL mode for concurrency; write operations are serialized via worker thread
- Context budget ensures no memory bloat in model input
- Summaries are used only when history exceeds thresholds
