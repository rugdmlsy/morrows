# Morrows Session Deduplication Audit

Task: `c259b8c8-c20a-4261-90be-bc9dd21f34d1`

## Target model

Morrows now uses four distinct identities:

- `Task`: the work identity.
- `Run`: one logical execution of that work.
- `Run.provider_conversation_ref`: the external Provider conversation/session/thread handle.
- `RuntimeScope`: the morrow-runtime machine execution and authorization scope.

Human↔Agent discussion is Task collaboration. A Task + AgentInstance uses `MessageThread(kind=human_agent)` and durable `Message` records. Morrows no longer creates its own Session ID.

## Removed active duplication

- Removed the Morrows `Session` core aggregate and active Store/Server implementation.
- Removed direct Session REST routes and the independent Session WebUI page.
- Removed employee `session_inbox`, `session_get`, `session_reply`, and Session summary MCP tools.
- Removed active Session summary/runtime paths and SessionRuntime credentials.
- Removed `LaunchAttempt.session_id`.
- Removed Provider conversation state from LaunchAttempt.
- Removed Session-derived Task write authorization.
- Removed SessionRuntime delivery claiming.
- Removed duplicate Agent author identity from active `Message`; `author_type + created_by` is authoritative.

## Merged responsibilities

- Task-bound historical Session transcripts migrate into `MessageThread` / `Message`.
- Human Interview now binds to `assignment_intakes.interview_thread_id`.
- Human messages use `AgentDelivery(kind=task_message)`.
- Agent replies use the same Task collaboration thread.
- Provider continuity is authoritative only in `Run.provider_conversation_ref`.
- LaunchAttempt remains process-attempt evidence only.

## Historical compatibility

Migration `0043_session_dedup.sql` keeps legacy `sessions`, `session_messages`, and `session_runtime_attempts` tables for audit/migration compatibility. They are not active product entities.

For Task-bound legacy Sessions, migration 0043:

- preserves the old Session UUID as the new Human/Agent thread UUID;
- preserves Human and Agent message IDs and content;
- migrates intake interview references to `interview_thread_id`;
- migrates the latest launch Provider reference to the Run;
- converts Task-bound `session_message` deliveries to `task_message`;
- removes obsolete SessionRuntime credentials from the active credential table;
- archives open legacy Session rows.

A migration-42 fixture test verifies the conversion and runs `PRAGMA foreign_key_check`.

## Other redundant or stale implementation found during the audit

- `begin_task_execution` called the same gate check twice. One duplicate call was removed.
- Run restart SQL still read deleted LaunchAttempt Session/provider fields. Restart now uses the latest LaunchAttempt only as process history and reads Provider continuity from the Run.
- Intake continuation still looked for old `session_message` deliveries. It now uses `task_message`.
- Active message authorship initially had both `created_by` and `author_agent_instance_id`. The duplicate field was removed.
- Old open Task Session presence previously granted Task write access. Messaging no longer affects authorization.

## Intentionally retained terminology

- Provider Session ID means an external Codex/Claude/CodeBuddy session/thread/conversation locator.
- The underlying morrow-runtime/LSM compatibility protocol still exposes legacy `/sessions/...` endpoint names and `logical_session_id` aliases. Morrows presents these as `RuntimeScope`. Renaming the underlying runtime protocol is a separate compatibility migration and is not needed for the Morrows data model.
- Legacy Session database tables remain read-only historical compatibility data.

## Verification

- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `cargo test --workspace`: passed; no failed suites or doctests.
- Managed intake E2E passes through Human Interview, Task message wake-up, Provider continuation, implementation, structured completion, and Task done.
- Provider restart test confirms successive LaunchAttempts reuse `Run.provider_conversation_ref`.
- Migration 42→43 compatibility test passes and preserves transcript/intake/provider continuity.
- WebUI `npm run build`: passed.
- WebUI `npm run lint`: 0 errors; 6 pre-existing React hook warnings remain.
- ASD-STE100 lint: 0 violations on the three core agent instruction files.
- ASD-STE100 lint: 0 violations across all 53 current MCP tool descriptions.
- Final schema has no Session/provider identity in `launch_attempts`.
- Final `agent_deliveries` accepts only `task_message` and `launch_instruction`.
- Final `agent_credentials` accepts only `runtime` and `bridge`.

## Result

The normal Morrows Task lifecycle no longer depends on a Morrows-owned Session identity. Task collaboration owns durable shared messages, Run owns Provider continuity, and RuntimeScope owns machine execution.
