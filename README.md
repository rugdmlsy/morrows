# Morrows

Local-first, agent-native work coordination for humans and multiple AI agent products/accounts.

The current local vertical slice separates company control from employee operations:

```
Human / Web UI / automation
          ↓ REST
   Morrows control plane
   ├─ registry / fleet
   ├─ dispatcher
   ├─ assignment / Run lifecycle
   └─ provider launch adapters
          ↓
        Agent
          ↓ employee MCP
   work / memory / collaboration / reporting
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
- Durable launch instructions, visible in the Web UI and over the employee MCP
- First-class direct Agent conversations with summary-only list loading and paged message history
- Conversation WebUI cache: history is fetched only after selection, then incrementally refreshed for the open chat
- Employee conversation inbox/read/reply MCP tools; human messages queue while the Agent runtime is idle
- External handoff adapters for LSM, Antigravity, and Gemini with owned accept/status
- Startup recovery for interrupted local launch jobs
- Optional LSM Run integration: one durable Logical Session per Run, scoped Codex MCP access, separate control API, execution evidence, explicit restart and bounded cleanup
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
# Optional loopback LSM integration
MORROWS_LSM_CONTROL_URL=http://127.0.0.1:8765
MORROWS_LSM_CONTROL_KEY=<same value as LOCAL_SHELL_MCP_CONTROL_API_KEY>
MORROWS_LSM_SUBJECT=local-mcp-client
MORROWS_AGENT_RESTART_GRACE_SECONDS=600
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

Morrows exposes a Streamable HTTP MCP endpoint for **employee operations**, not for company administration:

```
http://127.0.0.1:8787/mcp
```

Every MCP call uses the already-provisioned employee identity:

```
X-Agent-Instance-Id: <uuid>
```

The MCP surface intentionally does **not** expose agent/profile/account/machine registration, fleet state, capacity, dispatch policy, dispatch, assignment claiming, Run creation, launch profile management, launch/cancel operations, or dependency graph administration. Those remain control-plane responsibilities through REST/store/provider adapters.

Employee MCP tools currently cover:

- `conversation_inbox`, `conversation_get`, `conversation_reply`: receive and answer direct company conversations addressed to the caller.
- `work_request_submit`: submit a new work request without choosing priority, assignee, or launcher.
- `task_get`: read work owned by or assigned to the caller.
- `memory_get` / `memory_revise`: pull or revise durable task working memory. `memory_get` includes current context, collaboration records, and the caller's Run history.
- `instructions_get`: read management instructions attached to the caller's work.
- `artifact_create`, `decision_create`, `thread_create`, `message_create`: record work products and collaboration.
- `handoff_create`, `handoff_get`, `handoff_accept`, `task_collaboration`: continue work across employees without sharing provider chat history.
- `assignment_renew`, `run_checkpoint`, `run_complete`, `task_events`: maintain an existing assignment and report progress/completion.

Task-scoped MCP reads and collaboration writes verify that the caller owns the submitted request or has an Assignment history for that Task. MCP can report or collaborate on work, but it cannot create its own Assignment or Run.

Direct conversations are separate from Task collaboration. The WebUI loads only conversation summaries at startup; selecting a conversation loads the latest message page into an in-memory cache, older history is fetched explicitly, and only the selected conversation polls for new messages. Human messages are durable and remain queued until the addressed Agent replies. If that Agent already has a `starting`/`running` launch, Morrows also writes a lightweight launch instruction telling it to check the conversation inbox. Morrows does not keep an otherwise-idle Agent process alive solely for chat.

External adapters such as `lsm_external`, `antigravity_external`, `gemini_external`, and `codebuddy_external` remain compatibility launch backends. Their lifecycle endpoints are control-plane REST operations; they are no longer exposed as employee MCP tools. Provider-specific active launch adapters should be preferred when an automation API/CLI exists.

`MORROWS_*` settings take precedence; the former `AC_*` settings remain accepted during migration.

`X-Agent-Instance-Id` is still only an identity binding mechanism for the current local-only daemon, not remote authentication. A later milestone will replace it with issued credentials/tokens before remote exposure.

## Core invariant

Task identity is independent of model, account, machine, and conversation/session.

- **Task**: what work exists.
- **Assignment**: which AgentInstance currently owns a role, with a lease.
- **Run**: one concrete execution session.
- **ContextRevision**: immutable handoff/context snapshot.
- **Handoff**: explicit transfer from a source Run to a separately accepted target Run.
- **Message**: durable directed/reply-capable agent communication attached to a task thread.
- **Conversation / ConversationMessage**: direct human↔Agent communication independent of Task collaboration.
- **Artifact / Decision**: durable work evidence and rationale that can be referenced by handoff.
- **Event**: append-only audit record.

## Live handoff validation

M2.1 was exercised with two separate `codex-personal` CLI sessions and two distinct AgentInstances. Agent A created a typed artifact, decision, directed message, and pending handoff, which released A's assignment and ended A's Run as handed off. After the control plane assigned Agent B and created its Run, B reconstructed the state through employee MCP only, accepted the handoff, replied in the original thread, checkpointed the recovered state, and completed the task. No Codex session/chat history was shared between A and B.
