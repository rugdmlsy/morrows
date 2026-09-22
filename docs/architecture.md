# Architecture

## M1 scope

```
                       ┌──────────────┐
                       │ React Web UI │
                       └──────┬───────┘
                              │ REST
                              ▼
┌─────────────┐       ┌──────────────┐       ┌─────────────┐
│ MCP clients │──────▶│ ac-server    │──────▶│ ac-store    │
└─────────────┘       │ Axum + rmcp  │       │ SQLx/SQLite │
                      └──────┬───────┘       └─────────────┘
                             │
                             ▼
                       Domain invariants
```

REST and MCP call the same store/domain operations. MCP is not implemented as an HTTP-to-HTTP adapter.

## Implemented entities

```
Task
 ├── ContextRevision*
 ├── Assignment*
 │     └── Run*
 └── Event*
```

AgentInstance exists as a minimal M1 worker identity. AgentProfile / Account / Machine become separate first-class entities in M3.

## Important invariants

1. `claimed` is not a Task state.
2. Active ownership is represented by Assignment + lease.
3. One active Assignment exists per `(task, role)`.
4. Concurrent claims are resolved atomically by SQLite.
5. Run mutations require the same AgentInstance that owns the Assignment.
6. Context revisions are append-only/versioned.
7. Important actions append Events in the same transaction as state changes.
8. A successful `executor` Run may mark its Task done; non-executor roles do not.
9. Daemon restart does not erase Tasks, Runs, checkpoints, or pending durable jobs.

## Next implementation target

M2 adds Handoff, Artifact, Decision, MessageThread/Message, and TaskDependency, then validates:

```
Agent A → checkpoint → handoff → Agent B → continue
```

without sharing the original chat session.
