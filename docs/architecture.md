# Architecture

## Local architecture

```
                       ┌──────────────┐
                       │ React Web UI │
                       └──────┬───────┘
                              │ REST
                              ▼
┌─────────────┐       ┌──────────────┐       ┌─────────────┐
│ MCP clients │──────▶│ morrows-server    │──────▶│ morrows-store    │
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

AgentInstance remains the worker identity referenced by M1/M2 work. M3 links each instance to an AgentProfile and optional independent Account and Machine identities, with append-only CapacitySnapshots.

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

## M3 identity and capacity

An **AgentProfile** describes a stable product/template (`name`, `provider`, `kind`,
`default_capabilities`, `metadata`). An **Account** describes an independent provider
identity (`provider`, `label`, optional `external_account_ref`, `status`, `metadata`).
An account can be used with multiple profiles from the same provider. A **Machine**
describes a host (`name`, `hostname`, `os`, `arch`, `status`, `metadata`, `last_seen_at`).
All three have UUIDs and `created_at`/`updated_at` timestamps.

Registration creates or returns the existing row unchanged by these natural keys:
profile `(provider, name)`, account `(provider, label)`, machine `name`. Name/label must
be nonempty. Optional metadata defaults to `{}`, profile capabilities to `[]`, account
and machine status to `active`; unspecified machine descriptors and profile kind are
empty strings. No credentials or secrets are stored or requested.

**AgentInstance** keeps its original UUID, name, status, capabilities and heartbeat,
adding required `profile_id`, optional `account_id`, `machine_id`,
`external_instance_ref`, and `created_at`. Registration requires existing foreign keys;
nonempty account/profile providers must match exactly. Omitted capabilities inherit
profile defaults; explicit `[]` means no capabilities. The existing unique name remains
the registration key. Repeating normalized registration with the same links reuses the
UUID and refreshes status/capabilities/heartbeat; different links for an existing name
return a conflict rather than repointing the identity of historical work.

Migration `0004_identity_capacity.sql` adds columns in place and does not replace or
renumber instances, assignments, runs, messages, or handoffs. It backfills one explicit
`legacy` profile and per-instance legacy account/unknown-machine rows. Backfilled
account/machine UUIDs equal the corresponding instance UUID in their separate tables;
this makes migration deterministic without guessing shared real accounts or hosts.
The original heartbeat is used for backfilled creation time because M1 did not store
instance creation time. SQLite triggers enforce the required profile/time after
backfill, avoiding a rebuild of the heavily referenced instance table.

Legacy `agent_register(name, capabilities)` and `POST /api/agents` still return
AgentInstance. New legacy callers create/reuse the legacy profile/account and create an
explicit unknown host; repeat calls reuse the same instance and links. Refreshing a
normalized instance through legacy registration preserves its normalized links. Existing
`X-Agent-Instance-Id` configuration, M1 REST payload conventions, leases, run ownership,
and M2/M2.1 handoff/message semantics remain unchanged.

Heartbeat requires `X-Agent-Instance-Id` to equal the target instance UUID. In one
transaction it sets instance status and server heartbeat time, advances the linked
machine's `last_seen_at` without moving it backwards, and optionally appends capacity.
Invalid optional capacity leaves the entire heartbeat unchanged. Heartbeats do not
renew assignment leases or change runs, and status is an opaque nonempty label; there
is no automatic stale/offline classification or dispatch.

A **CapacitySnapshot** contains UUID, instance UUID, status, `available_slots`,
`active_assignments`, `active_runs`, optional `max_concurrency`, optional opaque string
`quota_state`, `details` (default `{}`), and server `observed_at`. Counts are reported
observations, not derived scheduling authority. All counts must be nonnegative and
available slots cannot exceed a supplied maximum. Capacity recording uses the same
ownership check as heartbeat but does not itself refresh heartbeat/status. Snapshots
cannot be updated or deleted, enforced in SQLite as well as the store API. History is
newest observation first, with insertion order breaking timestamp ties; latest is
`null` for a registered instance with no observations, and an unknown instance is an
error. No quota parsing, billing adapters, launcher, remote auth, or Dispatcher is added.

REST routes (under `/api`):

| Method | Path | Body / result |
| --- | --- | --- |
| POST / GET | `/agent-profiles` | Register profile / list profiles |
| GET | `/agent-profiles/{id}` | Profile |
| POST / GET | `/accounts` | Register account / list accounts |
| GET | `/accounts/{id}` | Account |
| POST / GET | `/machines` | Register machine / list machines |
| GET | `/machines/{id}` | Machine |
| POST / GET | `/agent-instances` | `{name, profile_id, account_id?, machine_id?, capabilities?, external_instance_ref?}` / list instances |
| GET | `/agent-instances/{id}` | Instance |
| POST | `/agent-instances/{id}/heartbeat` | `{status, capacity?}`; returns instance |
| POST / GET | `/agent-instances/{id}/capacity` | Capacity fields / full newest-first history |
| GET | `/agent-instances/{id}/capacity/latest` | Snapshot or `null` |
| GET | `/agent-fleet` | `[{instance, profile, account, machine, latest_capacity}]` |

Only heartbeat and capacity mutations above require the identity header; identity
registration remains available before a caller has an instance ID. Missing/invalid
headers and invalid counts return 400; cross-instance mutation returns 409; missing
identities return 404. Nullable account/machine links appear as `null` in fleet results.

MCP exposes `agent_profile_register/list/get`, `account_register/list/get`,
`machine_register/list/get`, `agent_instance_register`, `agent_heartbeat`,
`capacity_record/latest/history`, and `agent_fleet` (slash notation lists separate
tools). Identity registrations take the fields directly, and identity get tools take
`{id}`. Heartbeat/capacity recording takes `{agent_instance_id, input}` where `input`
matches the REST body. Capacity reads take `{agent_instance_id}`; lists and fleet have
no arguments. The request header supplies the actor, independently of the target ID.

Example heartbeat MCP arguments, using the returned instance UUID and matching header:

```json
{
  "agent_instance_id": "<instance-uuid>",
  "input": {
    "status": "online",
    "capacity": {
      "status": "available",
      "available_slots": 1,
      "active_assignments": 0,
      "active_runs": 0,
      "max_concurrency": 1,
      "quota_state": "unknown",
      "details": {"source": "worker-report"}
    }
  }
}
```

The Fleet UI polls the joined endpoint and shows profile, account, machine, instance
status, capabilities, heartbeat age, and latest capacity with its own observation age.
Missing capacity is shown explicitly rather than inferred as zero. M3 tests upgrade a
seeded M2.1 database using production migrations, compare all existing collaboration
rows, exercise the original run owner after migration, verify snapshot durability and
append-only constraints, and cover registration/ownership/validation through REST/MCP.

## M4 Dispatcher

M4 adds a durable, explainable assignment Dispatcher on top of the M3 identity and
capacity model. It does not launch, resume, or control external agent processes.

A `TaskDispatchPolicy` is keyed by task and role. It stores required capabilities,
optional AgentProfile / Account / Machine filters, heartbeat and capacity freshness
TTLs, assignment lease duration, and an enabled flag. Updating a policy is audited;
manual `task_claim` remains supported and unchanged.

`dispatch_preview` and actual dispatch use the same evaluator. For an executor task,
the evaluator requires a dispatchable task state, completed prerequisites, no live
assignment for that role, and an enabled policy. A candidate must be online, satisfy
all capability and optional identity filters, and have both a fresh heartbeat and a
fresh capacity observation. Capacity statuses `blocked`, `throttled`,
`unavailable`, or `offline` are rejected; quota states `usage_limited`,
`exhausted`, or `blocked` are also rejected.

CapacitySnapshot remains observation-only. To avoid oversubscribing from a stale but
still fresh snapshot, Dispatcher reconciles the reported values with current durable
Assignments:

`added_since_snapshot = max(current_active_assignments - observed_active_assignments, 0)`

`effective_slots = max(reported_available_slots - added_since_snapshot, 0)`

When `max_concurrency` is present, effective slots are additionally capped by
`max_concurrency - current_active_assignments`. Candidate order is deterministic:
highest effective slots, then fewest current active assignments, then freshest
capacity observation, then AgentInstance UUID.

`dispatch_task` and `dispatch_next` execute stale-lease cleanup, re-evaluation,
selection, Assignment creation, and audit writes inside SQLite `BEGIN IMMEDIATE`.
This serializes concurrent dispatch attempts so two callers cannot consume the same
last effective slot. Assignment expiry discovered by Dispatcher emits the same
`assignment.expired` audit event shape as the lease manager.

Each mutating attempt appends a `DispatchDecision` containing the full preview,
candidate reasons, outcome, selected instance when applicable, and resulting
Assignment. Decisions are append-only at the SQLite layer. Preview is read-only.
`dispatch_next` considers enabled policies by task priority descending, then task
creation time and UUID, recording no-candidate attempts until the first assignment.

REST additions under `/api`:

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/dispatch-policies` | List durable policies |
| POST | `/tasks/{id}/dispatch-policy` | Create or replace a policy |
| GET | `/tasks/{id}/dispatch-policy/{role}` | Read a policy |
| GET | `/tasks/{id}/dispatch-preview/{role}` | Explain task/candidate eligibility without mutation |
| POST | `/tasks/{id}/dispatch/{role}` | Atomically select and create an Assignment |
| GET | `/tasks/{id}/dispatch-decisions` | Read append-only decision history |
| POST | `/dispatch/next/{role}` | Dispatch the highest-priority eligible task |

MCP exposes the matching `dispatch_policy_list/set/get`, `dispatch_preview`,
`dispatch_task`, `dispatch_next`, and `dispatch_decisions` tools. These are
control-plane operations and do not require pretending the dispatcher is the selected
worker. The resulting Assignment is still owned by the selected AgentInstance, so
subsequent run mutations retain the existing identity checks.

The Work Queue marks tasks with enabled executor policies. Task Detail can edit the
basic capability/lease policy, preview concrete candidate rejection reasons, dispatch
explicitly, and inspect recent decisions. M4 deliberately defers executor launch
adapters, provider-specific quota parsers, preemption, remote authentication/RBAC, and
a periodic automatic dispatch loop.

## M5 Executor launcher

M5 consumes an existing executor Assignment without changing Dispatcher policy. An
operator registers a `LaunchProfile` for that AgentInstance. A local `codex_cli`
profile fixes the absolute executable and workspace; task text cannot choose the
program, cwd, environment, or argv. The bounded prompt is written to stdin.

`launch_enqueue` checks the live Assignment and profile in one immediate transaction.
For Codex, it creates a queued LaunchAttempt and a durable launch job. The worker
creates the Run before spawn, then starts either a new session:

`<program> exec --json --color never --approve-for-me -C <cwd> -o <last-message> [ -m <model> ] -`

or resumes a previous finished attempt for the same task, agent, and profile:

`<program> exec resume --json -o <last-message> [ -m <model> ] <session-id> -`

The worker records JSONL, stderr, last message, PID, and external session reference
under `MORROWS_LAUNCH_DIR/<attempt-id>/`. A `launch_stop` request atomically revokes
the assignment and marks the attempt cancelled; the owning worker polls that state
and kills its child. On restart, the daemon marks interrupted local attempts failed
and restores claimed jobs whose attempts had not started. It cannot safely identify
and kill an orphan process after an unclean OS crash.

`lsm_external`, `antigravity_external`, and `gemini_external` profiles contain no
executable or workspace. Enqueue writes an `awaiting_agent` attempt. The bound agent
polls `external_launch_list` over MCP and calls `external_launch_accept` with its
external session reference. Acceptance creates its Run atomically. The agent then
reads `task_get`, `context_get`, and `launch_instructions`, checkpoints progress,
and calls `run_complete` when done. The worker reconciles terminal Runs and expired
assignments. These adapters do not open a product UI or start a provider process.

`launch_instruction_send` creates a durable, ordered mailbox entry and audit event.
Agents read it with `launch_instructions`; a running Codex process must poll the MCP
tool, since the CLI has no live turn injection in this adapter. On resume, all
instructions are included in the new prompt.

Process exit is distinct from semantic completion. `run_complete` wins if already
committed. A zero exit without it pauses the Run, releases the Assignment, and
returns the Task to `ready`; a failed process marks the Run failed and does the same.

REST additions under `/api`:

| Method | Path | Purpose |
| --- | --- | --- |
| GET/POST | `/launch-profiles` | List/register launch profiles |
| GET | `/launch-profiles/{id}` | Read a profile |
| POST | `/launch-attempts/enqueue` | Queue Codex or invite an external agent |
| GET | `/launch-attempts/{id}` | Read attempt status |
| POST | `/launch-attempts/{id}/stop` | Stop and release an attempt |
| GET/POST | `/launch-attempts/{id}/instructions` | Read/send instructions |
| POST | `/launch-attempts/external/accept` | Accept as assigned AgentInstance |
| GET | `/agent-instances/{id}/external-launches` | Read external work for an instance |
| GET | `/tasks/{id}/launch-attempts` | Task launch history |
| GET | `/tasks/{id}/launch-instructions` | Task instruction history |

MCP exposes the corresponding profile, attempt, instruction, stop, external list,
and external accept tools. The Web Task Detail view supports launch/invite, resume,
instruction send, stop, and status. Dispatch and launch remain explicit separate
actions. Authentication and remote process launch belong to the later deployment
milestone.
