# Employee execution and persistence

## Start with enough information to act

Read `task_context`, then follow its continuation offsets. Confirm task goal,
project constraints, acceptance criteria, current context version, instructions,
handoffs, evidence and your assignment/Run. Discovery/read access is not an
assignment: the company control plane must assign the existing task and create a
Run before employee execution tools can be used. Use `task_request_assignment`
with the existing task ID, role and reason. It creates a durable request, not a
duplicate task or assignment. Track it with `assignment_request_list`; use
`include_resolved=true` for decisions and resulting assignment IDs. Exact pending
retries reuse the request. A changed reason requires withdrawing the pending
request, preserving both records. Only its author may withdraw it.

The company web task queue exposes pending requests, reasons, authors and history.
Its approve action creates/reuses an assignment atomically with request resolution;
it does not steal another worker's active role or start a Run. Operators can also
use `GET /api/assignment-requests` and
`POST /api/assignment-requests/{id}/resolve` with action `approve` or `reject`,
a `resolution` reason, and optional `lease_seconds`. An approved assignment then
uses the existing control-plane Run/launch workflow. A rejected request retains
the explanation. Pending requests grant no task-write rights.

A context can explain the background while still lack executable inputs. For an
audit task, locate the authoritative checkout and current Git/dirty state, source
baseline, original report and receipt URIs, reproducible fixture/recheck commands,
required environment, and current jobs. Check the timestamps and provenance of
imported knowledge. Missing artifact links cannot be inferred from a task title or
old statement that no jobs were running.

The shared capture instructions in
`crates/morrows-server/src/context_capture_instructions.md` are reused by MCP
initialization, executor launch prompts and direct Session prompts. They require
authorized imports to retain execution locations, source/evidence references,
observation times and unresolved inputs. A source summary can be older than its
latest activity; historical command metadata alone does not verify output content
or current filesystem state. An external Session ID does not grant access.

The launch prompt includes a bounded background excerpt and points to the full
`task_context`. A persisted ContextPackage retains the pinned full task context,
project background and all current shared memory IDs, as well as evidence IDs.
Without a handoff, its continuation text uses the author's current summary verbatim
and labels that source. Packages never include an employee's private memory or
manufacture verified results. These builders preserve stored information; they do
not automatically fetch private sources or reconstruct facts never saved to Morrows.

## What to save and when

Before substantive work, read applicable current project/organization knowledge;
a first page is not the entire collection. Proactively retrieve superseded history
when resolving conflicting observations, understanding an earlier decision or
constraint, revisiting a failed approach, or correcting a conclusion. Task context
history and Session history are different records. Shared prompts specify these
triggers, rather than injecting every historical body at startup.

At meaningful milestones and before handoff/completion, publish reusable facts,
decisions, failure lessons and corrections with provenance and uncertainty. Check
current entries first to avoid duplicates. Preserve useful unknowns as hypotheses;
do not promote them to verified facts. Project-only Sessions still need an
authorized source Task for project publication and can retain candidates in their
Session summary meanwhile. For broad search and native text editing, use the
[memory file CLI](memory-files.md); [multi-project Git management](memory-versioning.md)
is a separate, not-yet-migrated storage design.

| Record | When to use it | What it does not do |
| --- | --- | --- |
| `run_checkpoint` | Recoverable progress and run-local state at a meaningful milestone | Does not change shared context/project memory; keeps the latest checkpoint rather than all intermediate observations |
| `memory_revise` | Shared progress, blockers, next action, or changed goals/constraints; update before handoff | Does not extract facts from chat, validate research conclusions, or promote project memory |
| `artifact_create` | Immutable reports, receipts and other evidence with a resolvable URI | Does not verify the referenced content |
| `decision_create` | A chosen course and its rationale, including a candidate reusable finding linked to evidence | Does not automatically create long-term project knowledge |
| `project_memory_publish` | Explicitly publish reusable task findings to its project, with source context, basis, verification limits and evidence | Does not infer truth, publish organization memory, or erase a superseded entry |
| `handoff_create` | Work that another execution should continue; include completed/remaining/blockers and evidence IDs | Ends the source execution and releases its assignment; is not a completion claim |
| `run_complete` | Actual acceptance criteria are satisfied and evidence/context are saved | Does not evaluate the meaning of task constraints or independently verify evidence |

These persistence steps are employee workflow guidance. The server does not impose
a memory-update cadence or automatically infer missing updates. Run ownership,
lifecycle, active leases, write authorization and unfinished dependencies **are**
server-enforced. Completion additionally validates structured acceptance reports
as described below. Use checkpoint/context/handoff to stop incomplete work.

Project knowledge can be published directly by the task owner, any previously
assigned Agent, or the employee of an open Task Session. `project_memory_publish`
derives the destination project and author from the source task/authentication;
it cannot target another project or organization scope. Supply the current
`context_revision_id`, `verification_status` (reported, verified, hypothesis or
unverified), `basis`, and any source `artifact_ids`/`decision_ids`. All evidence
references must belong to the task; a verified claim requires at least one.
The server records provenance but does not certify the claim.

Use a stable `idempotency_key` per publication. An identical retry returns the
original entry; reuse with different content is rejected. A new revision uses a
new key plus `supersedes_memory_id` pointing to current shared knowledge in the
same project. Concurrent revisions cannot silently fork that entry. The original
content and provenance remain available with `project_get(include_superseded=true)`.
Organization knowledge remains control-plane managed. Store durable facts,
decisions and constraints; preserve uncertainty and source references.

## Completion contract

`run_completion_check` is a read-only preflight for an owned Run. Its
`completion_template`, exact original criteria and blockers provide the next
action without guessing a schema. For executor tasks with `acceptance_criteria`
or `freeze_requires` in the current context constraints, completion requires:

- The current context revision ID and a nonempty saved shared summary.
- Exactly one check for every criterion pointer, with status `passed`, rationale
  and at least one artifact ID belonging to this task and carrying a nonempty URI.
- Normal ownership, lease, lifecycle and unfinished-dependency checks.

Pass the report as `completion` to the MCP tool, or as `result.completion` (also
supported by the REST/CLI path). Extra result fields remain intact. Explicit
`result.ok=false` or `all_acceptance_criteria_met=false` blocks completion even
without structured criteria. A stale report, missing/duplicate/unknown criterion,
or foreign evidence prevents all state changes. `run_complete` repeats validation
under its writer transaction, so a successful preview cannot bypass newer context.

The server checks structure and reference ownership, not artifact contents or
scientific validity. Do not weaken constraints or label unknown checks passed.
Ungated tasks retain legacy completion behavior except explicit failure rejection.
These execution/persistence instructions are stored once in
`crates/morrows-server/src/execution_workflow_instructions.md` and reused by MCP
initialization, executor startup and direct Session prompts.

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
