# Employee discovery and context

All employee tools require an authenticated, registered Agent. Task and project
reads are company knowledge: an Agent may explicitly query another Agent's task
without taking ownership. Writes, assignment renewal, execution completion, and
private Session operations retain their existing authorization rules.

## Shared MCP instructions

Authentication, control-plane boundaries, read/write authorization, and the common
workflow live in one source: `crates/morrows-server/src/mcp_instructions.md`.
Morrows returns this text once in `initialize.result.instructions`. All employee
tools share it; `tools/list` descriptions contain only operation-specific guidance.
The repeated generic "Requires authenticated Agent identity" sentences are removed
from individual tools because authentication is defined by the shared instructions.

A client that copies server instructions into every model-visible tool description
reintroduces duplication outside Morrows. Fix that in the client/adapter by retaining
the shared instructions once; do not discard authentication or lifecycle information
from the server instructions to compensate. The HTTP smoke check verifies that
Morrows itself does not repeat the shared text in its tool descriptions.

## Start work

1. `whoami {}` identifies the connection without returning credentials.
2. `task_list {}` lists the caller's assigned unfinished work. This list is
   independent of whether an inbox delivery has already been acknowledged.
3. `task_context {"task_id":"..."}` reads fresh task/project background, the
   current context revision, current visible memory, five recent entries per
   collaboration section (handoffs, decisions, artifacts, dependencies), twenty
   instructions, and five assignment/caller-execution records. It does not write
   a snapshot or acknowledge deliveries.
4. Follow a section's `next_offset` using the tool named in `read_more`. Read
   `instructions_get` to acknowledge only instructions returned to their recipient.

`missing_context` reports absent task/project/context background and absent
`constraints.acceptance_criteria`. Acceptance criteria written only in free text
are preserved verbatim; the missing **structured** field is reported, not invented.
Readiness or verified completion is never inferred from a description or handoff.

## Discovery filters

```json
{"scope":"assigned", "agent_instance_id":"...", "project_id":"...", "limit":20, "offset":0}
{"scope":"all", "project_id":"...", "state":"blocked"}
{"scope":"all", "include_completed":true}
```

- `scope` defaults to `assigned`; `agent_instance_id` then defaults to the caller.
  `scope=all` removes the assignment filter and cannot be combined with an Agent ID.
- Assigned discovery uses the latest assignment per task role. Active and expired
  assignments remain discoverable; released handoffs and superseded assignments do
  not. Completed assignments are discoverable for terminal tasks when requested.
- Without `state`, done/cancelled tasks are excluded unless `include_completed=true`.
  An explicit state takes precedence.
- Task summaries include project names and 240-character description previews,
  with an explicit truncation flag. `task_get` returns the complete task text.
- Responses echo the caller and effective filters. Empty lists explain that the
  filter matched nothing; they do not imply that no other tasks exist.
- `project_list` pages all project summaries with 240-character description previews.
  `project_get` returns the complete project description and a page of current shared
  organization/project memory. Use `task_list(scope=all, project_id=...)` for its work.

## Paging and response compatibility

Pages use `limit` (default 20, range 1–100) and `offset` (default 0). A page contains
`items` and `next_offset` (`null` at the end). Lists are live; if the underlying data
changes while paging, restart the listing when an exact current inventory is needed.
Queries use database `LIMIT/OFFSET`, with one extra row to detect continuation.

- `task_events` is newest-first and supports an optional exact `event_type` filter.
- `task_collaboration` returns a page for each requested section. Omit `section` for
  all six sections or select `handoffs`, `artifacts`, `decisions`, `threads`,
  `messages`, or `dependencies`. Each section has its own continuation offset.
- `instructions_get` now returns a page rather than an array. Reading a page never
  acknowledges another Agent's delivery or an instruction on an unread page.
- `memory_get` returns `task_id`, current `context`, `long_term_memory`, and
  `next_offset`. It no longer repeats the entire task, collaboration, and execution
  history. Current knowledge excludes visible superseded revisions. The caller's
  own private Agent memory remains visible only to that caller.
- `task_get` retains task fields and adds `execution`: paged assignments and
  caller-owned execution records/checkpoints. Its page parameters apply to each
  execution collection. Other Agents' private provider-thread history is not read.
- `context_package_get` reads the latest persisted snapshot, or returns `null` if
  absent. It no longer creates durable state as a side effect of a read. Use
  `task_context` for fresh context and authorized `context_package_assemble` to
  explicitly persist a package. `context_matches_current` in `task_context` compares
  revision IDs only; it does not assert that every referenced record is up to date.

Existing callers that assumed full arrays/history must follow these page envelopes.
The control-plane REST endpoints retain their existing response formats.
