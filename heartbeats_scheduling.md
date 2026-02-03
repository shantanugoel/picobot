# Phase 3: Heartbeats & Scheduling - Final Implementation Plan

## Overview

Phase 3 introduces a job scheduling system that enables PicoBot to execute tasks on a schedule (interval or one-time), with heartbeat capabilities wired into the agent loop. This enables autonomous behaviors like periodic checks, reminders, and background monitoring tasks.

This plan incorporates a full security review. Scheduled runs must preserve Kernel invariants, avoid privilege escalation, and prevent duplicate execution under concurrency.

## Key Design Decisions

- Store a capability snapshot with each job to prevent privilege escalation.
- Route all scheduled execution through the Kernel to preserve schema validation, permission checks, and output wrapping.
- Treat scheduled prompts as user prompts by default (no system prompt privilege).
- Use atomic job claiming to prevent duplicate runs.
- Enforce per-user and global concurrency limits and quotas.
- Use exponential backoff for failures.
- Keep scheduler disabled by default (explicit opt-in).

## Scope

In scope for Phase 3:

- Interval and one-time schedules.
- Atomic job claiming with persistence.
- Execution through Kernel with capability snapshots.
- Execution caps, rate limits, and failure backoff.
- Scheduler service and background loop.
- Basic API integration for schedule management.

Deferred to Phase 3.1:

- Cron expression support.
- ScheduleTool (agent self-scheduling).
- Heartbeat-specific config section (treated as interval schedules for now).
- Notifications at least to whatsapp (if the user for which the task is added is registered with whatsapp module) and think of other that may be needed

## Architecture and Modules

New module structure:

```
src/scheduler/
├── mod.rs           # Module exports
├── job.rs           # Job definition, state, capability snapshot
├── executor.rs      # Background task executor (Kernel routed)
├── store.rs         # SQLite persistence with atomic claiming
├── error.rs         # Scheduler-specific errors
└── service.rs       # Scheduler service and background loop
```

## Database Schema

Add new tables to SQLite:

```
CREATE TABLE IF NOT EXISTS schedules (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    schedule_type TEXT NOT NULL CHECK(schedule_type IN ('interval', 'once')),
    schedule_expr TEXT NOT NULL,
    task_prompt TEXT NOT NULL,
    session_id TEXT,
    user_id TEXT NOT NULL,
    channel_id TEXT,
    capabilities_json TEXT NOT NULL,
    creator_principal TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    max_executions INTEGER,
    execution_count INTEGER NOT NULL DEFAULT 0,
    claimed_at TEXT,
    claim_id TEXT,
    claim_expires_at TEXT,
    last_run_at TEXT,
    next_run_at TEXT NOT NULL,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    backoff_until TEXT,
    metadata_json TEXT
);

CREATE INDEX IF NOT EXISTS idx_schedules_due ON schedules(next_run_at, enabled, claimed_at);
CREATE INDEX IF NOT EXISTS idx_schedules_user ON schedules(user_id);
CREATE INDEX IF NOT EXISTS idx_schedules_claim ON schedules(claim_id);

CREATE TABLE IF NOT EXISTS schedule_executions (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES schedules(id) ON DELETE CASCADE,
    started_at TEXT NOT NULL,
    completed_at TEXT,
    status TEXT NOT NULL CHECK(status IN ('running', 'completed', 'failed', 'timeout', 'cancelled')),
    result_summary TEXT,
    error TEXT,
    execution_time_ms INTEGER
);

CREATE INDEX IF NOT EXISTS idx_schedule_executions_job ON schedule_executions(job_id, started_at);
```

## Job Definitions

- Store a capability snapshot with each job.
- Track ownership and principal type for auditability.
- Track failures with backoff to prevent runaway retries.

Core types live in `src/scheduler/job.rs`:

- `ScheduledJob`
- `ScheduleType` (Interval, Once)
- `Principal` (User, System, Admin)
- `JobExecution`
- `ExecutionStatus`
- `CreateJobRequest`

## Atomic Claiming and Concurrency

Use `BEGIN IMMEDIATE` and `UPDATE ... WHERE` to atomically claim due jobs.

- `claim_due_jobs()` selects only jobs claimed by the current worker.
- `complete_job()` updates `next_run_at` and clears claim fields.
- `fail_job()` increments failure count and sets exponential backoff.

Claiming fields:

- `claimed_at`
- `claim_id`
- `claim_expires_at`

## Execution Through Kernel

Scheduled execution must preserve Kernel invariants:

- Build a Kernel using the job's capability snapshot.
- Use normal agent loop methods so tool validation and permission checks occur.
- Treat the scheduled prompt as a user prompt unless explicitly authorized.

Timeouts must use a CancellationToken and be propagated to model/tool execution.

## Scheduler Service

The `SchedulerService` is the main entry point and owns:

- Job creation and validation (with per-user quotas).
- Background tick loop and job claiming.
- Cancelling running jobs (token cancellation).

Execution flow:

1. Tick loop claims due jobs atomically.
2. Executor runs each job with concurrency limits.
3. Execution recorded in `schedule_executions`.
4. Job is completed or failed with backoff.

## Permissions and Security

Add a new permission type:

- `Permission::Schedule { action: String }`

Rules:

- Schedule creation requires explicit schedule permission.
- Scheduled runs can only execute with the captured capability snapshot.
- Prompts are treated as user input by default.

## Configuration

Add a scheduler config section:

```
[scheduler]
enabled = false
tick_interval_secs = 1
max_concurrent_jobs = 4
max_concurrent_per_user = 2
max_jobs_per_user = 50
max_jobs_per_window = 100
window_duration_secs = 3600
job_timeout_secs = 300
max_backoff_secs = 3600
```

## Testing Strategy

Unit tests:

- Atomic claiming and claim expiry.
- Next run calculation for interval and once.
- Capability snapshot enforcement.
- Per-user quota enforcement.
- Exponential backoff limits.

Integration tests:

- End-to-end job execution and persistence.
- Restart recovery without duplicate execution.
- Cancellation and timeout handling.
- Permission regression (no privilege escalation).

Security tests:

- Malicious prompts do not grant extra permissions.
- Jobs created without schedule permission are rejected.

## Implementation Order

1. Create `src/scheduler/` module structure and error types.
2. Add job types with capability snapshot.
3. Implement `ScheduleStore` with atomic claiming.
4. Implement `JobExecutor` routing through Kernel.
5. Implement `SchedulerService` with quotas and backoff.
6. Add `Permission::Schedule` in `src/kernel/permissions.rs`.
7. Add scheduler config in `src/config.rs`.
8. Integrate scheduler startup in server runtime.
9. Add tests (unit and integration).
10. Add execution history recording and basic API endpoints.

## Dependencies

- `tokio-util` with CancellationToken support.
- `dashmap` for per-user semaphore map.

## Items To Handle After Completing This Plan

- Phase 3.1: cron expression support.
- Phase 3.1: ScheduleTool for agent self-scheduling.
- Phase 3.1: heartbeat-specific config and UX.
- Phase 3.1: Add job chaining and workflows.
- Phase 3.1: Add structured metrics and tracing for scheduler runs.
- Phase 3.1: Extend operational controls (pause, rescan, cancel, delete schedules).
