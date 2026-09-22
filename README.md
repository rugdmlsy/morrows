# Agent Company Work OS

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
- SQLite job/outbox tables reserved for durable automation

Not yet implemented: automatic Dispatcher, executor launch adapters, multi-user auth/RBAC, or distributed deployment.

## Run

From the repository root:

```bash
cd web
npm install
npm run build
cd ..

cargo run -p ac-server
```

Then open:

```
http://127.0.0.1:8787
```

Environment variables:

```bash
AC_DATABASE_URL=sqlite://data/agent-company.db
AC_BIND=127.0.0.1:8787
AC_WEB_DIR=web/dist
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
