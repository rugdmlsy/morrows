# Architecture

## Local architecture

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
 ├── Handoff*
 ├── Artifact*
 ├── Decision*
 ├── MessageThread*
 │     └── Message*
 ├── TaskDependency*
 └── Event*
```

AgentInstance is currently the worker identity used by M1/M2. AgentProfile / Account / Machine become separate first-class entities in M3.

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

## Collaboration continuation

M2 adds Handoff, Artifact, Decision, MessageThread/Message, and TaskDependency, then validates:

```
Agent A → checkpoint → handoff → Agent B → continue
```

without sharing the original chat session.

## M2 collaboration

M2 adds append-only Artifact (title, URI, description, kind defaulting to `other`), Decision (title and rationale),
MessageThread/Message, and structured Handoff records. Artifact URIs are references;
the daemon does not fetch or execute their contents. TaskDependency edges point from
work to its prerequisite. Self-links, duplicates, and cycles are rejected. Executor
claims and completion require all prerequisites to be done; other roles can collaborate
while prerequisites are unfinished. Dependency removal is explicit and audited.

A handoff contains summary, completed work, remaining work, blockers, and artifact/decision
references. It pins the current immutable ContextRevision and requires at least one remaining
work item. Creating it requires a live source run and an unexpired assignment owned by the
caller. In one SQLite write transaction, it validates task-scoped references, saves the handoff,
marks all live runs on the source assignment `handed_off`, releases that assignment, and
appends `handoff.created`. The task remains unfinished. The next agent reads the handoff,
claims the released role, starts its own run, and explicitly accepts the handoff. There is no dispatcher or agent launcher.

Run start, checkpoint, completion, renewal, and handoff serialize ownership checks with
writes. A released or expired session cannot subsequently complete the task. Context
revision creation and dependency cycle checks also serialize their read/write transactions.

REST additions (under `/api`):

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/tasks/{id}/collaboration` | All task collaboration records, including messages and prerequisites |
| POST | `/tasks/{id}/artifacts` | Create `{title, uri, description?, kind?}` |
| POST | `/tasks/{id}/decisions` | Create `{title, rationale}` |
| POST | `/tasks/{id}/threads` | Create `{title}` |
| POST | `/threads/{id}/messages` | Append `{body, message_type?, recipient_agent_instance_id?, recipient_role?, reply_to_message_id?, correlation_id?, requires_response?, status?}` |
| POST | `/runs/{id}/handoffs` | Create `{summary, completed, remaining, blockers?, artifact_ids?, decision_ids?}` |
| POST | `/handoffs/{id}/accept` | Accept with `{target_run_id}` |
| GET | `/handoffs/{id}` | Read handoff, pinned context, and referenced artifacts/decisions |
| POST | `/tasks/{id}/dependencies` | Add `{depends_on_task_id}` |
| DELETE | `/tasks/{id}/dependencies/{dependency}` | Remove prerequisite |

All new REST mutations require `X-Agent-Instance-Id` identifying a registered instance,
matching the existing MCP identity convention. This local identity header is not a new
credential/authentication system. M1 REST payload conventions remain compatible.

MCP exposes `artifact_create`, `decision_create`, `thread_create`, `message_create`,
`handoff_create`, `handoff_get`, `handoff_accept`, `task_collaboration`, `dependency_add`, and
`dependency_remove`. Create tools accept the relevant `task_id`, `thread_id`, or `run_id`
and a typed `input` object matching the REST body. Dependency tools take `task_id` and
`depends_on_task_id`; `handoff_get` takes `handoff_id`. Mutations use the same identity
header and store operations as REST. The Task Detail UI displays all six entity types.

Continuation sequence:

1. Agent A claims a task, starts a run, and writes a checkpoint and task context.
2. A creates artifacts/decisions and calls `handoff_create` with remaining work.
3. Agent B calls `task_collaboration`, then `handoff_get` to recover the pinned context.
4. B claims the same role, starts a new run, accepts with `handoff_accept(handoff_id, target_run_id)`, and completes the remaining work.

Integration tests cover this sequence across database reopen, rollback on invalid references,
concurrent handoffs and reverse dependency edges, stale ownership, dependency gating,
non-executor completion, and REST/MCP adapter behavior. Existing M1 tests remain in place.

M2.1 adds optional directed recipients (registered agent instance and/or role), same-thread
reply links, and correlation IDs to messages. Message type defaults to `note`, status to
`sent`, and requires_response to false; these describe the stored message and do not
automatically dispatch work or mark requests answered. Type/status/kind are nonempty
string labels. Replies must reference an existing message in the same thread.

Handoffs start `pending` with no accepting run. Acceptance changes status to `accepted`
and sets `accepted_by_run_id` in the same transaction as `handoff.accepted`. It requires
a running or paused target run on the same task, owned by the caller, with an active,
unexpired assignment. Duplicate or concurrent acceptance has exactly one winner.
The REST body uses `target_run_id`; MCP takes `handoff_id` and `target_run_id`.
The additive 0003 migration preserves existing M2 records with the defaults above.

## Live CLI validation

M2.1 was validated against the persistent local daemon with two independent `codex-personal`
CLI sessions bound to two different AgentInstance identities. Agent A created collaboration
records and a pending handoff, ending its Run and releasing its Assignment. Agent B had no
access to A's Codex conversation; it recovered the task via `task_get`, `context_get`,
`task_collaboration`, and `handoff_get`, then claimed executor, started a distinct Run,
accepted the handoff, replied to A's directed message with the same correlation ID, checkpointed
the reconstructed state, and completed the task. The persisted handoff's
`accepted_by_run_id` points to B's Run, and the reply's `reply_to_message_id` points to A's
request.
