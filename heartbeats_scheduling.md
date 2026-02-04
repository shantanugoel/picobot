# Phase 3: Heartbeats & Scheduling - Final Implementation Plan

## Overview

Phase 3 completes the core job scheduling system in PicoBot and Phase 3.1 extends it with cron, self-scheduling tools, notifications, and heartbeat configuration. All scheduled execution must preserve Kernel invariants, prevent privilege escalation, and avoid duplicate execution under concurrency.

## Current Status

Core scheduler infrastructure is implemented:

- Job types with capability snapshots.
- SQLite-backed schedule store with atomic claiming.
- Job executor routing through Kernel.
- Scheduler service with quotas and backoff.
- Permission::Schedule capability.
- Scheduler config and runtime startup.
- Basic API routes for create/list.

Remaining work is primarily API surface completion plus tests, then Phase 3.1 features.

## Key Design Decisions

- Capability snapshots are stored per job and used for execution (no privilege drift).
- Schedule creation requires explicit schedule permissions.
- Scheduled prompts are treated as user prompts by default.
- Atomic job claiming prevents duplicate execution.
- Global and per-user concurrency limits are enforced.
- Exponential backoff prevents retry storms.
- Scheduler remains disabled by default (explicit opt-in).

## Part A: Complete Phase 3 (Remaining Work)

### A1. API Routes

Add the remaining schedule management endpoints:

- GET /api/v1/schedules/:id
- DELETE /api/v1/schedules/:id
- PATCH /api/v1/schedules/:id
- GET /api/v1/schedules/:id/executions
- POST /api/v1/schedules/:id/cancel

Implementation details:

- Enforce ownership on every route.
- DELETE should cancel any running job before removing the schedule.
- PATCH updates affect the next run only (running executions continue unchanged).
- Add ScheduleStore method: list_executions_for_job(job_id, limit, offset).

### A2. Tests (Priority Ordered)

P0: Atomic claim recovery and concurrency

- expired claim is reclaimed
- no duplicate execution on restart
- concurrent workers claim disjoint jobs

P0: Capability snapshot security

- job cannot exceed snapshot capabilities
- malicious prompt does not escalate permissions
- schedule creation requires permission

P1: Cancellation and lifecycle

- cancel running job (done)
- timeout marks job as timeout (done)
- delete cancels then removes schedule (done)

P1: End-to-end execution

- interval job reschedules correctly (done)
- once job disables after execution (done)
- execution recorded in history (done)

## Part B: Phase 3.1

### B1. Cron Expression Support

- Add ScheduleType::Cron.
- Use the cron crate for parsing and next occurrence.
- Enforce minimum interval (>= 60s) to prevent overload.
- Support explicit timezone, default to UTC.
- Update schedule_type validation in persistence.

### B2. ScheduleTool (Agent Self-Scheduling)

Add a new tool allowing the agent to create, list, delete, and cancel schedules.

Security rules:

- New schedules must inherit a strict subset of current session capabilities.
- Enforce policy limits (minimum interval, max execution count, quotas).
- Permission::Schedule action gating applies for each operation.

### B3. Notifications (Async + Durable)

Add a notification subsystem with a durable queue:

- NotificationChannel trait with per-channel delivery.
- notifications table for delivery status, retries, and errors.
- NotificationService worker processes pending deliveries.
- WhatsApp notifier integrates with existing backend.
- user_contacts table stores verified channel addresses.

Job execution should enqueue notifications without blocking job completion.

### B4. Heartbeat Configuration

Add heartbeats config section:

```toml
[heartbeats]
enabled = true
default_interval_secs = 300

[[heartbeats.prompts]]
name = "health_check"
prompt = "Check system health and report issues"
interval_secs = 60

[[heartbeats.prompts]]
name = "daily_summary"
prompt = "Generate daily summary"
cron = "0 0 18 * * *"
timezone = "America/New_York"
```

On startup, auto-register heartbeat schedules as system principals.

### B5. Metrics and Tracing

- Add low-cardinality metrics for job counts and durations.
- Add tracing spans with job_id and user_id for debugging (not as metric labels).

## Priority Order and Effort

P0 (highest priority):

1. Claim recovery and capability security tests.
2. Cron support with minimum interval enforcement.
3. ScheduleTool with capability subset enforcement.

P1:

1. Complete API routes (CRUD + history + cancel). (done)
2. Cancellation and lifecycle tests. (done)
3. Async notifications with WhatsApp integration. (done)

P2:

1. Heartbeat config. (done)
2. Metrics and tracing. (done)

## Files to Create or Modify

Create:

- src/tools/schedule.rs
- src/notifications/mod.rs
- src/notifications/channel.rs
- src/notifications/queue.rs
- src/notifications/whatsapp.rs
- src/notifications/service.rs
- tests/scheduler_claiming.rs
- tests/scheduler_security.rs
- tests/scheduler_lifecycle.rs
- tests/scheduler_integration.rs

Modify:

- src/scheduler/job.rs
- src/scheduler/service.rs
- src/scheduler/store.rs
- src/scheduler/executor.rs
- src/server/scheduler_routes.rs
- src/tools/builtin.rs
- src/config.rs
- src/main.rs
- Cargo.toml

## Dependencies

- cron (latest)
- chrono-tz
- metrics
- metrics-exporter-prometheus (optional)
