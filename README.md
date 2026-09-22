# Agent Company Work OS

Local-first, agent-native work coordination for humans and multiple AI agent products/accounts.

The M1 vertical slice is intentionally small:

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
- SQLite job/outbox tables reserved for durable automation

Not yet implemented: Handoff, MessageThread, Artifact, Decision, full AgentProfile/Account/Machine split, automatic Dispatcher, executor launch adapters.

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

Register an AgentInstance using the `agent_register` tool. For mutation tools such as `task_claim`, `assignment_renew`, `run_start`, `run_checkpoint`, and `run_complete`, send the returned UUID in:

```
X-Agent-Instance-Id: <uuid>
```

The server obtains actor identity from the transport request instead of trusting an `agent_id` supplied in the tool arguments.

This header is an identity binding mechanism for the local-only M1 daemon, not authentication. A later milestone will replace it with issued credentials/tokens before remote exposure.

## Core invariant

Task identity is independent of model, account, machine, and conversation/session.

- **Task**: what work exists.
- **Assignment**: which AgentInstance currently owns a role, with a lease.
- **Run**: one concrete execution session.
- **ContextRevision**: immutable handoff/context snapshot.
- **Event**: append-only audit record.
