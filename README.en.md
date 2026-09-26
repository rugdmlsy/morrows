# Morrows

[简体中文](README.md) | **English**

Morrows is a local-first, agent-native work coordination system for humans and multiple AI agent products, accounts, and runtime environments.

The current local vertical slice separates company control from employee operations:

```text
Human / Web UI / automation
          ↓ REST
   Morrows control plane
   ├─ registry / fleet
   ├─ dispatcher
   ├─ assignment / execution lifecycle
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
- Long-term `MemoryEntry` records scoped to organization / project / Agent / task, with provenance and supersede chains
- Durable Artifact / Decision / MessageThread / Message / Handoff / TaskDependency records
- Directed agent messages with reply/correlation metadata
- Atomic handoff creation and explicit acceptance linked to the accepting Run
- Dependency cycle prevention and executor gating
- Normalized AgentProfile / Account / Machine / AgentInstance identities
- Owned heartbeats, append-only capacity observations, and joined Agent Fleet UI
- Durable per-task dispatch policies and append-only dispatch decisions
- Explainable capacity-aware Dispatcher with atomic Assignment creation
- Durable executor LaunchProfile / LaunchAttempt records and background launch jobs
- Safe Codex CLI launch, Session resume, stop, and Run/Session reconciliation
- Durable launch instructions, visible in the Web UI and over the employee MCP
- First-class Agent Sessions with summary-only list loading and paged message history
- Session WebUI cache: history is fetched only after selection, then incrementally refreshed for the open Session
- Employee Session inbox/read/reply MCP tools backed by a durable Agent delivery outbox
- Issued/revocable Agent Bearer credentials; runtime credentials are short-lived and Run-bound, bridge credentials are explicitly issued by the local control plane
- Issued/revocable control-plane Operator credentials with `viewer` / `operator` / `admin` RBAC; WebUI can persist an Operator token locally
- External handoff adapters for LSM, Antigravity, and Gemini with owned accept/status
- Startup recovery for interrupted local launch jobs
- Optional LSM Run integration: one durable Logical Session per Run, scoped Codex MCP access, separate control API, execution evidence, explicit restart and bounded cleanup
- Chinese/English Web UI with Chinese as the first-visit default
- SQLite durable jobs for launcher work and a transactional Agent delivery outbox with provider resume
- Manual Codex Agent provisioning uses the account email plus a target-machine credential reference such as `~/.codex` or `~/.codex-personal`; Morrows never uploads, copies, or stores Codex `auth.json` or provider login tokens
- One private authentication directory per managed Codex Account; credential files are not stored in the Morrows database or committed to Git
- Reusable local deployment script `scripts/deploy.sh` for build, tmux restart, and health verification

Not yet implemented: direct process control for all external agent products, interactive multi-user accounts/SSO, built-in TLS termination, or distributed deployment. External adapters invite an existing agent Session; they do not open those products automatically.

## Run

From the repository root, local development can still be started directly:

```bash
cd web
npm install
npm run build
cd ..

cargo run -p morrows-server
```

Production is deployed to the OVH VPS by default. After committing and pushing, run locally:

```bash
./scripts/deploy-vps.sh
```

The script synchronizes `main` on the VPS, builds the Web UI and release server, and
restarts `morrows.service`. Production startup also verifies a deployment fingerprint written
only by this flow: the Git commit, server binary, `web/dist`, `run-vps.sh`, and installed
systemd unit must all match. A manual `git pull/reset`, rebuild, frontend replacement, or unit
replacement followed by restart exits with status 78 and prints:

```text
ERROR: refusing to start an unmanaged Morrows production deployment.
Deploy with: ./scripts/deploy-vps.sh
```

A normal restart of an unchanged deployed build remains valid. Morrows itself listens only on VPS loopback
`127.0.0.1:8787`; the public MCP endpoint is exposed through the existing Cloudflare
Tunnel and VPS routing layer at:

```text
https://mcp.xycdev.com/morrows
```

The same instance exposes the WebUI at:

```text
https://mcp.xycdev.com/morrows/ui/
```

Public control-plane API calls require an Operator Bearer credential; the WebUI top bar can persist that token.

Local Shell MCP on the VPS moves to `127.0.0.1:8766`. A loopback router owns
`127.0.0.1:8765`: only `/morrows` is sent to Morrows and every other path continues
to Local Shell MCP, so the existing `https://mcp.xycdev.com/mcp` endpoint is unchanged.
This shared Caddy edge is owned by `deploy/morrow/deploy-vps.sh` in the
`local-shell-mcp` repository; Morrows does not maintain a second router copy.

For local development, the previous tmux deployment remains available:

```bash
./scripts/deploy.sh
./scripts/deploy.sh --fast
```

The local development address remains `http://127.0.0.1:8787`.

Environment variables:

```bash
MORROWS_DATABASE_URL=sqlite://data/morrows.db
MORROWS_BIND=127.0.0.1:8787
MORROWS_WEB_DIR=web/dist
MORROWS_LAUNCH_DIR=data/launches
MORROWS_MCP_URL=https://mcp.xycdev.com/morrows
# rmcp Host validation automatically allows the hostname from MORROWS_MCP_URL.
# Add any extra Hosts as a comma-separated list when needed:
# MORROWS_MCP_ALLOWED_HOSTS=internal.example:9443

# Optional loopback LSM integration
MORROWS_LSM_CONTROL_URL=http://127.0.0.1:8766
MORROWS_LSM_SUBJECT=local-mcp-client
MORROWS_AGENT_RESTART_GRACE_SECONDS=600
# On the VPS, do not copy MORROWS_LSM_CONTROL_KEY manually.
# scripts/run-vps.sh reads only LOCAL_SHELL_MCP_CONTROL_API_KEY from the
# private LSM service.env and maps it at process startup.

# Optional on loopback; required for any remote-facing deployment
MORROWS_REQUIRE_AGENT_AUTH=1
MORROWS_REQUIRE_OPERATOR_AUTH=1

# Bootstrap admin credential used only to mint the first durable admin token.
# Remove it from the environment after bootstrapping.
MORROWS_BOOTSTRAP_OPERATOR_TOKEN=mrw_operator_<secret>

# Direct non-loopback plaintext HTTP remains blocked by default.
# Prefer loopback + TLS reverse proxy. Development override only:
MORROWS_ALLOW_INSECURE_REMOTE_HTTP=0
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

Morrows exposes a Streamable HTTP MCP endpoint for **employee operations**, not company administration:

```text
http://127.0.0.1:8787/mcp
```

Employee MCP calls support issued Bearer credentials:

```text
Authorization: Bearer mrw_agent_<secret>
X-Agent-Instance-Id: <uuid>   # optional with Bearer; if present it must match
```

The local control plane can issue/list/revoke bridge credentials with `/api/agents/{id}/credentials` and `/api/agent-credentials/{id}/revoke`. Provider launchers use separate Run-bound runtime credentials and revoke them when the launch attempt finishes. Set `MORROWS_REQUIRE_AGENT_AUTH=1` to reject the legacy identity-header-only path even on loopback.

The MCP surface intentionally does **not** expose agent/profile/account/machine registration, fleet state, capacity, dispatch policy, dispatch, assignment claiming, Run creation, launch profile management, launch/cancel operations, or dependency graph administration. Those remain control-plane responsibilities through REST/store/provider adapters.

## Control-plane Operator auth

Control-plane REST supports issued `mrw_operator_*` Bearer credentials. `viewer` can read control-plane state; `operator` can mutate ordinary work/dispatch/launch state; `admin` additionally manages identities and Agent/Operator credentials.

Bootstrap access uses `MORROWS_BOOTSTRAP_OPERATOR_TOKEN` as temporary admin authority; after issuing a durable admin credential, remove the bootstrap token from the environment and restart.

Credential management endpoints are:

- `GET/POST /api/operator-credentials`
- `POST /api/operator-credentials/{id}/revoke`

Only hashes are stored in SQLite and list responses never return token/hash material. The WebUI top bar can store one Operator token in browser local storage and attaches it to `/api/*` control-plane requests.

Employee MCP tools currently cover:

- `session_inbox`, `session_get`, `session_reply`: receive and answer Agent Sessions addressed to the caller.
- `work_request_submit`: submit a new work request without choosing priority, assignee, or launcher.
- `task_get`: read work owned by or assigned to the caller.
- `memory_get` / `memory_revise`: pull or revise durable task working memory. `memory_get` includes relevant organization/project/Agent/task long-term `MemoryEntry` records, current context, collaboration records, and the caller's Run history.
- `instructions_get`: read management instructions attached to the caller's work.
- `artifact_create`, `decision_create`, `thread_create`, `message_create`: record work products and collaboration.
- `handoff_create`, `handoff_get`, `handoff_accept`, `task_collaboration`: continue work across employees without sharing provider chat history.
- `assignment_renew`, `run_checkpoint`, `run_complete`, `task_events`: maintain an existing assignment and report progress/completion.

Task-scoped MCP reads and collaboration writes verify that the caller owns the submitted request or has an Assignment history for that Task. MCP can report or collaborate on work, but it cannot create its own Assignment or Run.

A Session is an independent durable conversation object with an optional scope: a **general Session** is unbound, a **project Session** belongs to a Project, and a **task Session** belongs to a Task (its Project is derived from the Task). The WebUI loads only Session summaries at startup; selecting a Session loads the latest message page into an in-memory cache, older history is fetched explicitly, and only the selected Session polls for new messages.

Human messages are durable and remain awaiting reply until the addressed Agent replies. A separate transactional `AgentDelivery` exposes only two user-facing delivery states: **awaiting delivery** and **delivered**; delivered means Morrows successfully wrote the message into an Agent runtime prompt. Queued messages can be recalled before runtime claim.

The WebUI can explicitly start/resume a local Agent CLI for a Session through a dedicated Session runtime that binds AgentInstance, Account, LaunchProfile, Morrows Session, and the persisted provider thread/session reference without creating a fake Task or Run. Task launches also bind to the open Session for that Task + Agent (creating one when necessary) and share the same Provider-thread continuity with direct Session runtimes. Run / LaunchAttempt records describe execution lifecycle rather than a separate conversation identity. General Session messages are never consumed by Task launches and cannot wake unrelated interrupted Runs.

External adapters such as `lsm_external`, `antigravity_external`, `gemini_external`, and `codebuddy_external` remain compatibility launch backends. Their lifecycle endpoints are control-plane REST operations; they are no longer exposed as employee MCP tools. Provider-specific active launch adapters should be preferred when an automation API/CLI exists.

`MORROWS_*` settings take precedence; the former `AC_*` settings remain accepted during migration.

`X-Agent-Instance-Id` remains an identity binding hint, not a secret. Issued Agent and Operator Bearer credentials are stored only as SHA-256 hashes, and the two credential classes cannot cross their authorization planes.

Morrows still refuses direct non-loopback plaintext HTTP by default even when both auth systems are strict, because Bearer credentials require transport security. For remote use, keep Morrows on loopback and expose it through a TLS reverse proxy; `MORROWS_ALLOW_INSECURE_REMOTE_HTTP=1` exists only for isolated development.

## Three-plane architecture and core recovery invariant

Morrows cleanly decouples into three planes:

1. **Morrows Control Plane**: source of truth for work semantics, assignments, context snapshots, long-term memory, decisions, and artifacts.
2. **LSM Runtime Plane**: source of truth for host execution resources, isolated runtime scopes, jobs, shells, and execution audit.
3. **Provider Plane**: source of truth for model-private rollout Sessions, tool representations, and reasoning traces. Provider Sessions are private and never directly shared.

> **Ultimate Recovery Invariant**
>
> Task identity is independent of model, account, machine, Morrows Session, and Provider Thread. Even if the original Agent process, Provider Session, execution machine, and temporary workspace directories disappear completely, a new Agent can reconstruct full state and proceed solely from Morrows canonical records.

### Normalized naming model

To avoid ambiguity from bare words such as `session` and `run`, Morrows uses collaborative Chinese user-facing terminology together with precise system-layer definitions:

| User-facing term | Formal system English | Historical code alias | Definition |
| :--- | :--- | :--- | :--- |
| **工作项** | `WorkItem` | `Task` | An objective unit of work in the organization with its own lifecycle |
| **工作分配** | `WorkAssignment` | `Assignment` | Time-bounded responsibility, with a Lease, held by an Agent instance for a specific role in a WorkItem |
| **工作执行** | `WorkExecution` | `Run` | One logical execution of a WorkAssignment by an Agent |
| **启动记录 / 启动尝试** | `AgentLaunch` | `LaunchAttempt` | A concrete attempt to start an Agent process in order to advance a WorkExecution |
| **运行空间** | `RuntimeScope` | LSM `Logical Session` | LSM's isolated execution and authorization scope for one WorkExecution |
| **执行任务** | `RuntimeJob` | LSM `Job` | A concrete asynchronous machine task inside a RuntimeScope, such as `cargo test` |
| **持久终端 / 终端** | `PersistentShell` | `shell session` | A long-lived command-line environment inside a RuntimeScope |
| **浏览器实例** | `BrowserInstance` | `browser session` | A controlled stateful browser instance inside a RuntimeScope |
| **模型会话** | `ProviderThread` | `Provider Session` | A provider-private continuous model conversation, such as Codex / Claude / Gemini |
| **会话** | `Session` | `Conversation` | A durable one-to-one working conversation with a specific AgentInstance; it may be general-, project-, or task-scoped and spans multiple runtime/Run attempts |
| **上下文快照** | `ContextSnapshot` | `ContextRevision` | Frozen work background, goals, and memory view used to initialize a WorkExecution |
| **记忆** | `Memory` | `Memory` / `Context` | Durable knowledge across tasks and Sessions, scoped to organization/project/Agent/task |
| **成果物** | `Artifact` | `Artifact` | A formally archived and globally referenceable work product |
| **决策记录** | `Decision` | `Decision` | A confirmed technical or business decision that constrains or guides later work |
| **工作交接** | `Handoff` | `Handoff` | A structured protocol transferring work responsibility from one execution to another |
| **执行证据** | `WorkExecutionEvidence` | `RunExecutionEvidence` | Traceable evidence linking work semantics to LSM Audit / Job Logs |

Full specifications:

- [docs/agent-work-and-context-spec.md](docs/agent-work-and-context-spec.md)
- [docs/architecture.md](docs/architecture.md)

## Live handoff validation

M2.1 was exercised with two separate `codex-personal` CLI Sessions and two distinct AgentInstances.

Agent A created a typed artifact, decision, directed message, and pending handoff, which released A's Assignment and ended A's Run as handed off. After the control plane assigned Agent B and created its Run, B reconstructed the state through employee MCP only, accepted the handoff, replied in the original thread, checkpointed the recovered state, and completed the task.

No Codex Session/chat history was shared between A and B.
