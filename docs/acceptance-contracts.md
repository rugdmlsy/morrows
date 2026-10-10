# Native Task acceptance contracts

A Task's immutable acceptance_criteria is its completion contract. Work requests,
REST Task creation, and the WebUI can supply criteria. Description text alone
does not create a gate. Empty contracts keep the existing completion behavior.

Each criterion has a stable id, requirement, verification mode,
allow_not_applicable, optional required_artifact_kinds and
requires_independent_review. Mark every criterion requires_independent_review
to require review of the entire Task, or mark selected criteria. Definitions
cannot be replaced by context updates. A changed native requirement needs a
genuine rework Task. Current criteria/version are present in Task reads and
assembled ContextPackage summaries.

## Verification and evidence

- self_attested validates the executor's PASS, rationale and same-Task artifacts.
- deterministic_check executes the fixed command, machine, cwd and timeout
  through the existing owned Run RuntimeScope.
- artifact_check reads the fixed absolute path on the fixed machine through
  that RuntimeScope. This checks existence/basic readability, not correctness.
- independent_review requires a different AgentInstance with a reviewer
  Assignment and Run. Claim with role=reviewer, then read review_context and
  submit criterion_review. The input includes the reviewed context revision and
  acceptance version. The shared review view excludes private provider handles
  and checkpoints. Evidence IDs must match the final completion submission.
- human_review is an operator-only REST/WebUI verdict. It records authenticated
  operator identity and requires the reviewed context/version.

The publisher's check.read_only is a declaration of intended effect scope,
not a shell sandbox. Checks must have no external writes. Morrows enforces its
existing authorized Run/runtime/worker policy; this feature adds no privileges.
A fixed command does not prove that referenced test scripts were unmodified.
Use independent review to assess the implementation, tests and evidence.

criterion_verify accepts only a Run ID, criterion ID and optional retry reason.
It cannot accept an alternate command/path/worker or client-provided PASS.
The server reserves the attempt before execution and saves command/target,
scope, exit/output, timestamps and retry rationale. Nonzero exit, timeout,
runtime errors and missing success data cannot PASS.

Every repeated attempt needs a repair/environment-change reason. A lost response
is BLOCKED; there is no automatic replay. An operator must first inspect the
first operation's outcome, then POST to
/api/tasks/{task}/verification/reconcile with receipt_id, reason, and
observed_outcome_confirmed=true. This preserves the original evidence and marks
only FAIL, permitting a new actual check. It cannot manufacture PASS.

## Gate and lifecycle

run_completion_check returns criteria, criterion_statuses, missing_evidence,
blockers and the completion template. result.completion supplies one check per
criterion_path (/acceptance_criteria/<stable-id>), context_revision_id (null
when no context exists), PASS/FAIL/BLOCKED/NOT_APPLICABLE, rationale and
artifact_ids. NOT_APPLICABLE needs both explicit criterion permission and a
rationale. A receipt from another Run, Task, version or context is ineligible.
A later runtime verification attempt invalidates earlier independent PASS
review so repaired state receives another review.

run_complete repeats all gates inside the lifecycle write transaction. Failed
acceptance leaves Task and executor Run live. Reviewer FAIL returns the Task to
in_progress; a reviewer PASS places it in review. The executor repairs and
submits new evidence/review. A reviewer Run can complete without completing the
Task. Only successful executor completion marks done and unlocks successors.

CompletionRecord is saved atomically with completion: criteria snapshot/version,
per-criterion verdicts and references, all check attempts and review verdicts,
waivers, unresolved items, timestamp, actor/Run and Project Memory disposition.
Task/API/WebUI reads explain why completion was accepted. Pending UI statuses
come from server readiness; historical receipts are displayed as history.

## Legacy compatibility

Context acceptance_criteria/freeze_requires are imported to the same Task model.
Their original JSON-pointer criterion paths and old passed evidence remain
valid. The legacy context-write interface still updates imported contracts
and increments the contract version; native criteria never use this path.
Legacy declarations and self_attested modes remain structural attestation,
not independent proof. Existing done Tasks are not re-completed or given
fabricated retrospective CompletionRecords.

## Employee interfaces

New MCP tools: criterion_verify, review_context, criterion_review,
verification_receipts. Reviewer acquisition uses task_claim(role=reviewer) on
open unfinished Tasks. Deterministic verification belongs to the implementing
executor Run; reviewers have no arbitrary runtime privilege.

Operator REST: /tasks/{id}/verification, /runs/{id}/verify,
/runs/{id}/review, /tasks/{id}/human-review and the reconciliation endpoint.
Operator submissions record their provenance rather than claiming that an
operator action originated from a reviewer credential.

