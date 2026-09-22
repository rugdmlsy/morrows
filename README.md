# Morrows

Local-first, agent-native work coordination for humans and multiple AI agent products/accounts.

The current local vertical slice is:

```
Human Web UI
    ↓
Task
    ↓
MCP Agent
    ↓
Assignment lease
    ↓
Run
    ↓
Checkpoint
    ↓
Complete
    ↓
Append-only event timeline
```

## Current implementation

- Rust + Tokio
- Axum HTTP server
- SQLite + SQLx migrations
- React + Vite Web UI, served by the Rust daemon in production mode
- MCP Streamable HTTP server using `rmcp 3.x`
- Durable Task / Assignment / Run / ContextRevision / Event records
- Atomic concurrent task claiming
- Assignment lease renewal and automatic expiry scan
- Run ownership checks at the store/domain boundary
- Immutable context revisions
- Durable Artifact / Decision / MessageThread / Message / Handoff / TaskDependency records
- Directed agent messages with reply/correlation metadata
- Atomic handoff creation and explicit acceptance linked to the accepting Run
- Dependency cycle prevention and executor gating
- Normalized AgentProfile / Account / Machine / AgentInstance identities
- Owned heartbeats, append-only capacity observations, and joined Agent Fleet UI
- Durable per-task dispatch policies and append-only dispatch decisions
- Explainable capacity-aware Dispatcher with atomic Assignment creation
- Durable executor LaunchProfile / LaunchAttempt records and background launch jobs
- Safe Codex CLI launch, session resume, stop, and Run/session reconciliation
- Durable launch instructions, visible in the Web UI and over MCP
- External handoff adapters for LSM, Antigravity, and Gemini with owned accept/status
- Startup recovery for interrupted local launch jobs
- Chinese/English Web UI with Chinese as the first-visit default
- SQLite durable jobs for launcher work; outbox remains reserved for later delivery automation

Not yet implemented: automatic dispatch→launch chaining, direct process control for the external agent products, multi-user auth/RBAC, or distributed deployment. External adapters invite an existing agent session; they do not open those products automatically.

## Run

From the repository root:

```bash
cd web
npm install
npm run build
cd ..

cargo run -p morrows-server
```

Then open:

```
http://127.0.0.1:8787
```

Environment variables:

```bash
MORROWS_DATABASE_URL=sqlite://data/morrows.db
MORROWS_BIND=127.0.0.1:8787
MORROWS_WEB_DIR=web/dist
MORROWS_LAUNCH_DIR=data/launches
```

## Test

```bash
cargo test --workspace
cd web && npm run build
```

or:

```bash
./scripts/check.sh
```

## MCP

Streamable HTTP endpoint:

```
http://127.0.0.1:8787/mcp
```

Register explicit identities with `agent_profile_register`, `account_register`, and `machine_register`, then use `agent_instance_register` to link a worker. Legacy `agent_register(name, capabilities)` remains supported and returns the same AgentInstance UUID on refresh. See [M3 identity and capacity](docs/architecture.md#m3-identity-and-capacity) for fields, REST routes, and compatibility details. For mutation tools such as `task_claim`, `assignment_renew`, `run_start`, `run_checkpoint`, and `run_complete`, send the returned UUID in:

```
X-Agent-Instance-Id: <uuid>
```

The server obtains actor identity from the transport request instead of trusting an `agent_id` supplied in the tool arguments.

M4 adds `dispatch_policy_list/set/get`, `dispatch_preview`, `dispatch_task`, `dispatch_next`, and `dispatch_decisions`. Dispatch creates an Assignment only; it deliberately does not start or resume an external agent process.

M5 keeps that boundary and adds explicit launch operations: `launch_profile_register/list/get`,
`launch_enqueue`, `launch_attempt_get`, `task_launch_attempts`, `launch_stop`,
`launch_instruction_send`, and `launch_instructions`. The `codex_cli` adapter uses an
operator-controlled program and workspace, sends task text through stdin, captures logs and
session IDs, and can resume a previous finished attempt for the same task/agent/profile.
Stopping revokes the assignment and kills the child process owned by this daemon. A normal
process exit without `run_complete` returns the task to `ready`.

`lsm_external`, `antigravity_external`, and `gemini_external` have no executable path. Enqueue
creates an `awaiting_agent` invitation. The matching AgentInstance reads
`external_launch_list` through MCP, calls `external_launch_accept` with its session reference,
then uses the existing Run checkpoint/complete tools. It can read new instructions with
`launch_instructions`; the daemon reconciles completed Runs and expired assignments. These
adapters track work across product accounts and sessions without claiming a provider-specific
automation interface that is not available.

`MORROWS_*` settings take precedence; the former `AC_*` settings remain accepted during migration.

This header is an identity binding mechanism for the local-only daemon, not authentication. A later milestone will replace it with issued credentials/tokens before remote exposure.

## Core invariant

Task identity is independent of model, account, machine, and conversation/session.

- **Task**: what work exists.
- **Assignment**: which AgentInstance currently owns a role, with a lease.
- **Run**: one concrete execution session.
- **ContextRevision**: immutable handoff/context snapshot.
- **Handoff**: explicit transfer from a source Run to a separately accepted target Run.
- **Message**: durable directed/reply-capable agent communication attached to a task thread.
- **Artifact / Decision**: durable work evidence and rationale that can be referenced by handoff.
- **Event**: append-only audit record.

## Live handoff validation

M2.1 was exercised with two separate `codex-personal` CLI sessions and two distinct AgentInstances. Agent A created a typed artifact, decision, directed message, and pending handoff, which released A's assignment and ended A's Run as handed off. Agent B reconstructed the state through MCP only, claimed the task, started a new Run, accepted the handoff, replied in the original thread, checkpointed the recovered state, and completed the task. No Codex session/chat history was shared between A and B.
