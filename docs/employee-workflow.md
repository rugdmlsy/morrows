# Employee execution and persistence

## Start with enough information to act

Read `task_context`, then follow its continuation offsets. Confirm task goal,
project constraints, acceptance criteria, current context version, instructions,
handoffs, evidence and your assignment/Run. Discovery/read access is not an
assignment: the company control plane must assign the existing task and create a
Run before employee execution tools can be used. Do not create a duplicate work
request merely to obtain execution rights to an existing task.

A context can explain the background while still lack executable inputs. For an
audit task, locate the authoritative checkout and current Git/dirty state, source
baseline, original report and receipt URIs, reproducible fixture/recheck commands,
required environment, and current jobs. Check the timestamps and provenance of
imported knowledge. Missing artifact links cannot be inferred from a task title or
old statement that no jobs were running.

## What to save and when

| Record | When to use it | What it does not do |
| --- | --- | --- |
| `run_checkpoint` | Recoverable progress and run-local state at a meaningful milestone | Does not change shared context/project memory; keeps the latest checkpoint rather than all intermediate observations |
| `memory_revise` | Shared progress, blockers, next action, or changed goals/constraints; update before handoff | Does not extract facts from chat, validate research conclusions, or promote project memory |
| `artifact_create` | Immutable reports, receipts and other evidence with a resolvable URI | Does not verify the referenced content |
| `decision_create` | A chosen course and its rationale, including a candidate reusable finding linked to evidence | Does not automatically create long-term project knowledge |
| `handoff_create` | Work that another execution should continue; include completed/remaining/blockers and evidence IDs | Ends the source execution and releases its assignment; is not a completion claim |
| `run_complete` | Actual acceptance criteria are satisfied and evidence/context are saved | Does not evaluate the meaning of task constraints or independently verify evidence |

These persistence steps are employee workflow guidance. The server does not impose
a memory-update cadence or automatically infer missing updates. Run ownership,
lifecycle, active leases, write authorization and unfinished dependencies **are**
server-enforced. Completing an executor Run marks the task done even if a supplied
result says an acceptance criterion failed; the employee must not use it to stop
incomplete work. Use checkpoint/context/handoff instead.

Project and organization long-term knowledge have a control-plane write API, but
no direct employee MCP write tool. Record a candidate finding with source evidence
in task artifacts/decisions and request promotion through an authorized company
workflow. This is not automatic promotion or a built-in review queue. Store durable
facts, decisions and constraints, not raw logs or unsupported conclusions.

## Safe task-memory updates

`memory_revise` patches the latest context atomically and creates a new immutable
revision. Omitted fields (and top-level nulls) retain their current values. Supplied
strings replace that field; constraint objects merge recursively. Unmentioned keys
survive; explicitly supplied arrays, scalars and null replace that key's value.
An empty constraint object does not erase existing rules. The control-plane full
revision API retains its explicit replacement semantics.

```json
{
  "task_id": "...",
  "expected_context_revision_id": "revision read by the employee",
  "current_summary": "Located original receipt; recheck has not run. Next: validate fixture command and environment."
}
```

If the expected revision is stale, no state is written. Read the latest context,
reconcile the newer evidence and retry. Without an expected ID, the patch merges
against the latest revision under the writer lock; omitted information is still
preserved, but the employee should use the expected ID when its update depends on
what it previously read. Legacy non-object constraints survive summary-only
updates; converting their structure requires the full control-plane revision API.

## Simulation boundary

Read production source data, then perform lifecycle writes on a disposable task
and database with explicit simulation provenance. Do not acknowledge production
deliveries, claim the real task, run its experiments, write fake research findings,
or call its completion endpoint merely to test usability.
