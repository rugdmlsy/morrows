# Task Revision: versioned, auditable contract changes

Morrows separates a Task's effective contract from a proposed replacement.
Task/Assignment/Run IDs do not change when a Task is revised. Existing evidence,
check receipts, completion records, milestones, and provider conversations are
preserved, not rewritten.

## Canonical data and versioning

- `tasks.spec_version` identifies the active contract; the Task's
  `acceptance_criteria_json` and `acceptance_version` are the actual acceptance
  gate. A TaskRevision row is an immutable old/new contract *snapshot* plus
  mutable, audited proposal resolution metadata.
- `task_revisions` preserves original version 1 and full before/after snapshots,
  author, reason, diff (computed on read), context IDs, timestamps, acknowledgment
  Run/Agent, impact analysis and updated plan. Each revision gets a stable ID and
  monotonically increasing version, even after rejection.
- A TaskSpec includes title, description, goal, scope and formal acceptance.
  Goal/scope are projected to a new ContextRevision on application; scope is
  saved as `constraints.task_scope`. ContextRevision history is immutable and
  retained. Existing ContextPackages are not rewritten: they are stale until
  a fresh intake/assembly.
- Only the original publishing AgentInstance or authenticated operator may
  propose. Task executor ownership alone does not authorize editing.
  The server—not caller-supplied JSON—determines the publisher's identity.
- `expected_version` is mandatory CAS; proposals reject a stale base version,
  terminal Task, no-op patch, invalid acceptance contract, or a second pending
  revision. Changes to context while pending cause acknowledgment to reject
  until the proposal is rejected and re-proposed against the new context.
- Ready/backlog Tasks without any live Run apply the new contract atomically.
  An in-progress/review Task or a Task with a live Run enters `pending_ack`.
  The executor reads the full diff and submits nonempty impact and updated plan.
  Only the live implementing executor Run with a valid lease can acknowledge.
  Rejection is recorded with reason and leaves previous effective criteria
  in force. Canceling a Task rejects its pending proposal atomically.
  Already completed Tasks use rework, not Task Revision.

## Runtime and acceptance boundaries

A pending proposal does **not** forcibly cancel in-flight shells, jobs or
external side effects. It gates the next controlled operation: new verification,
human/independent review and executor completion. Actual outside actions must
reach their own safe point. Notification should never be described as instant
interruption.

Application occurs under a single SQLite IMMEDIATE writer transaction. It
updates Task spec/acceptance version and ContextRevision together, repins live
Run ContextRevision metadata after executor acknowledgment, and records the
revision-applied event. Verification receipts, historical failures and old
reviews remain available to audit. Completion and verification filter receipts
by the *new* acceptance_version, context_revision_id and Run, so old PASS
cannot be recycled. A new verification attempt after revision is not
considered a retry of the old contract. Completion validation holds a writer
transaction; a concurrent proposal cannot silently slip past it.

The durable `task_message` delivery is written **in the same transaction** as
`pending_ack` to the latest executor Assignment (active or expired), using the
existing Human/Agent Task collaboration thread. No artificial Run or
Session is created. Off-line agents can read `delivery_inbox`,
`task_context.task_revision`, `task_get.task_revision`, or
`task_revision_history` after resuming; a successor executor can acknowledge
with their own Run. All resolution transitions also appear in Task events.

## API

Employee MCP:

- `task_revision_history(task_id)` returns full versioned history.
- `task_revision_preview(task_id,draft)` previews old/new diff.
- `task_revision_propose(task_id,draft)` submits with publisher identity.
- `task_revision_ack(task_id,ack)` applies a pending revision after replan.
- `task_revision_reject(task_id,revision_id,reason)` rejects a pending proposal.

`draft` has `expected_version`, `reason` and optional
`title`, `description`, `goal`, `scope`, `acceptance_criteria`.
Omitted fields stay unchanged. An explicit empty criteria list removes all
criteria (still audited); acceptance validation checks supplied definitions.
An `ack` contains `revision_id`, `run_id`, `impact`,
`updated_plan` (a nonempty list). The caller's AgentInstance is trusted
from authentication; a different worker cannot acknowledge.

Operator REST:

- `GET /api/tasks/{id}/revisions`
- `POST /api/tasks/{id}/revisions/preview`
- `POST /api/tasks/{id}/revisions`
- `POST /api/tasks/{id}/revisions/{revision_id}/reject`

All writes require operator control-plane authorization. REST deliberately
does not impersonate an executor for acknowledgment; use that executor's MCP
tool. The WebUI provides preview/submit with old/new diff, status, full
history and executor impact/plan. On pending revisions it explains the
safe-point semantics, and the proposed criterion changes are visually
distinguished from the effective current contract.

CLI over authenticated employee MCP:

```sh
morrows task-revision TASK_UUID history
morrows task-revision TASK_UUID preview --input proposed.json
morrows task-revision TASK_UUID propose --input proposed.json
morrows task-revision TASK_UUID ack --input acknowledgment.json
morrows task-revision TASK_UUID reject --input rejection.json
```

`proposed.json` contains the draft; `acknowledgment.json` contains
revision_id/run_id/impact/updated_plan; `rejection.json` contains
revision_id/reason. CLI credential access is the ordinary employee MCP,
not raw production database mutation.

## Compatibility and limits

Migration 0046 adds tables and the effective spec version without resetting
Task state or deleting existing records; baseline revision snapshots are
backfilled after legacy acceptance import. Earlier pre-feature edits
cannot be reconstructed beyond the authoritative state at migration time.

Structured receipt and review correctness is protected by acceptance/context
versioning. Self-attestation remains a declaration; it is not an independent
content proof. Delivery indicates a queued message, not that the executor
has read it or that an external operation was interrupted. A successor must
read and acknowledge before new verification/completion.

Source changes must be made and tested on Mac, then committed/pushed; the VPS
only receives deploys through scripts/deploy-vps.sh.
