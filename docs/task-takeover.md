# Executor takeover (Morrows lifecycle)

An authenticated employee AgentInstance uses task_takeover_status and
task_takeover with task_id, reason, optional request_id and lease_seconds to
recover an **open**, **in_progress** Task whose previous executor lease has
expired or whose executor is absent. The server decides eligibility.
Status is advisory; the write transaction rechecks all guards.

Takeover obtains BEGIN IMMEDIATE, revokes the expired Assignment, terminalizes
old running/paused executor Runs as abandoned with reason takeover, creates a
fresh executor Assignment (context_review) and fresh intake Run, and appends a
durable audit event. It does not rebind the old Run, nor delete checkpoints,
milestones, artifacts, messages, or the Task/Project binding. The successor
must read current Project Memory with task_intake, then start/finalize Human
Interview and call task_begin_execution before executing. Missing predecessor
ContextRevision or milestone does not prevent recovery. Existing cooperative
handoff continues to reopen the Task via its ready/reconciliation lifecycle.

Run status and Assignment status fence old Run completion, checkpoint writes,
renew, and Task-owned runtime access. Runtime access also checks the lease
deadline server-side. Competing successors cannot both win; the exclusive
transaction and unique active-role index implement atomic single ownership.

A client-supplied request_id makes retry idempotent per authenticated actor:
same Task, successor, reason and force flag return the original Assignment and
Run; changing payload with the same key is rejected. The WebUI reuses a key
only for the same attempt; change any form field for a new key.

## Safety and permissions

Lease expiry does NOT prove the old process has stopped. Takeover is blocked
when a managed RuntimeScope is nonterminal, runtime provisioning is
unacknowledged, job waits are pending/ready/queued, a managed launch is
starting/running, or Run cleanup is unresolved. Have the runtime operator
quiesce/reconcile it first. This release does not silently kill running jobs.
External work performed outside Morrows RuntimeScope cannot be fenced by
Morrows: successors must independently recheck resource state before retrying
non-idempotent effects.

Employee self-takeover is open-Task-only and rejects unexpired active owner.
Approval and dispatch are never implicitly converted to open; the normal
executor/reviewer separation, dependency gate, and intake checks apply.
No project read permission is treated as execution authorization.

The authenticated control-plane GET /api/tasks/{id}/takeover returns advisory
status, owner, latest Run/milestone summary, eligibility and reason. Admin-only
POST /api/tasks/{id}/takeover accepts agent_instance_id, reason, force,
request_id, lease_seconds. Force can preempt an active lease, but cannot
override runtime/job quiescence. Viewer and Agent credentials cannot access the
Admin write route. The WebUI requires confirmation and an audited reason.

## Verification

The Rust store and full workspace tests cover contextless legacy intake,
expired takeover, active-owner and cleanup refusals, idempotent request
replays, concurrent double-claim, old Run fencing, orphan in_progress,
runtime scope blocking, and approval-mode preservation. A disposable local
Morrows server with two different real Bearer Agent credentials exercises
native HTTP JSON-RPC MCP claim/status/takeover/intake and persisted SQLite
audits. A real Chrome browser calls authenticated control-plane REST through
the WebUI, including Admin forced transfer and desktop/mobile layout.

Original ARP Task Oracle 396db6e2-1f99-4b6f-be8e-1eac5ac28035 was
already taken by Codex and completed. Task
81401f31-d040-4fe4-86e4-66747428c947 was already taken by Codex and
actively executing. They were inspected read-only; no one revoked their
Assignment or modified their Task/Run history to manufacture a test.

Mac is the only authoritative source worktree. Deploy using the existing
scripts/deploy-vps.sh only after pushing main. Never modify VPS production
SQLite or source files directly.
