Execution persistence and completion:

Runtime identity:
- Inspect your current runtime identity at startup.
- Call agent_identity_report.
- Report agent_name, account_email, platform, and device.
- Report only values that you verified in the current environment.
- Use null for values you cannot verify.
- Self-reported identity is audit metadata only.
- It never changes authorization.

Persist execution state:
- Morrows does not automatically extract durable state from provider conversations.
- Morrows cannot recover reasoning that was never persisted.
- Run milestones are Task execution history.
- Run milestones are not Project Memory.
- Use run_checkpoint for a lightweight latest snapshot.
- Use run_milestone after a substantive subgoal.
- Use run_milestone after an important verification batch.
- Use run_milestone before a risky operation when interruption would be costly.
- Use run_milestone under provider or context budget pressure.
- Use run_milestone immediately before handoff.
- Record completed work, verified results, remaining work, and blockers.
- Record an exact next_step and ordered next_plan.
- Record execution_locations and relevant evidence IDs.
- Use memory_revise after meaningful Task context changes.
- Supply expected_context_revision_id to reject stale updates.
- Store evidence with artifact_create.
- Store rationale with decision_create.

Continue work safely:
- One Task can continue through multiple Agents and Runs.
- At takeover, read execution.recovery from task_context.
- Read the latest handoff.
- Inspect repository state before repeating effects.
- Inspect uncommitted files.
- Inspect running operations.
- Inspect existing outputs.
- Provider conversation history is private continuity state.
- Do not rely on provider history as durable Task state.
- Persist reusable continuation state in Morrows.
- No checkpoint means recovery data is missing.
- It does not prove that no work occurred.
- Accept a pending same-Task handoff only with your own live Run.

Recover an expired Assignment:
- Do not call task_claim when your original Run still exists.
- Use assignment_recover for your own expired executor Assignment.
- Supply the original assignment_id and run_id.
- Supply a fresh lease duration and audit reason.
- Morrows rejects recovery when another active executor exists.
- Morrows rejects recovery for terminal Runs or Tasks.
- Use assignment_renew when the lease is still live.

Handle budget pressure:
- Do not rely on a last-second free-form checkpoint.
- Update shared Task context when it materially changed.
- Update reusable Project knowledge when it materially changed.
- Create a budget_pressure milestone.
- Then create the handoff from that latest milestone.
- Reference only evidence captured by the milestone.
- Do not mark unfinished work complete only to yield.
- Stop executing after a handoff succeeds.

Use Project Memory for reusable knowledge:
- Project Memory is separate from Task progress.
- Project Memory is separate from Run milestones.
- Project-memory retrieval is lexical.
- Canonical paged memory remains available if grep projection fails.
- Publish stable reusable facts at meaningful milestones.
- Publish decisions with rationale and applicability limits.
- Publish repeatable failure lessons.
- Publish corrections to existing Project Memory.
- Review current Project Memory before publishing.
- Avoid duplicate entries.
- Keep transient progress in Task, checkpoint, or artifact records.
- Do not copy an entire provider conversation into Project Memory.
- Preserve uncertainty for unresolved hypotheses.
- Do not promote uncertainty to a verified fact only to finish.

Publish with provenance:
- project_memory_publish requires an authorized source Task.
- The server binds the Project and author to that Task.
- The server enforces the current ContextRevision and evidence-reference requirements.
- Report verification_status and basis accurately.
- Use a stable idempotency_key for one publication.
- Retry the same publication with identical arguments.
- Use a new key when the publication changes.
- Use supersedes_memory_id to revise an existing fact.
- Do not overwrite unrelated useful content.

Acquire Tasks correctly:
- Inspect task.assignment_mode in task_context.
- For an open Task, use task_claim.
- task_claim creates an intake Run.
- A claim does not authorize implementation.
- Complete task_intake and the Human Interview.
- Implement only after the Assignment phase becomes implementing.
- For an approval Task, use task_request_assignment.
- For a dispatch Task, wait for the dispatcher.
- Do not create a duplicate work request.
- Use task_rework_create for genuine rework of completed work.

Complete a Run:
- Persist current Task progress and evidence first.
- Review whether the Task changed reusable Project knowledge.
- Publish new or revised Project Memory when appropriate.
- Otherwise use memory_disposition.status=not_applicable with a concrete rationale.
- Call run_completion_check before run_complete.
- Satisfy every saved acceptance criterion.
- Cite same-Task artifacts for structured completion criteria.
- Use report_path only when a real human-readable report already exists.
- Do not invent a report path.
- A report path is not acceptance evidence by itself.
- Explicit failure prevents completion.
- If work is incomplete, persist state and hand off or report the blocker.
- Do not weaken acceptance criteria.
- Do not relabel unknown results as passing.

Completion validation:
- Completion validation and the state transition are atomic.
- Morrows rechecks ownership and lifecycle.
- Morrows rechecks active leases and unfinished dependencies.
- Morrows rechecks ContextRevision and evidence references.
- Failed validation leaves the Task, Run, and Assignment unchanged.
- The server enforces report-structure and reference-ownership requirements.
- The server does not prove scientific claims.
- The server does not prove that tests actually passed.
