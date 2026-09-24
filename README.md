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
- Employee conversation inbox/read/reply MCP tools backed by a durable Agent delivery outbox
- Issued/revocable Agent Bearer credentials; runtime credentials are short-lived and Run-bound, bridge credentials are explicitly issued by the local control plane
- Issued/revocable control-plane Operator credentials with `viewer` / `operator` / `admin` RBAC; WebUI can persist an Operator token locally
- External handoff adapters for LSM, Antigravity, and Gemini with owned accept/status
- Startup recovery for interrupted local launch jobs
- Optional LSM Run integration: one durable Logical Session per Run, scoped Codex MCP access, separate control API, execution evidence, explicit restart and bounded cleanup
- Chinese/English Web UI with Chinese as the first-visit default
- SQLite durable jobs for launcher work and a transactional Agent delivery outbox with provider resume

Not yet implemented: direct process control for all external agent products, interactive multi-user accounts/SSO, built-in TLS termination, or distributed deployment. External adapters invite an existing agent session; they do not open those products automatically.

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

Morrows exposes a Streamable HTTP MCP endpoint for **employee operations**, not for company administration:

```
http://127.0.0.1:8787/mcp
```

Employee MCP calls support issued Bearer credentials:

```
Authorization: Bearer mrw_agent_<secret>
X-Agent-Instance-Id: <uuid>   # optional with Bearer; if present it must match
```

The local control plane can issue/list/revoke bridge credentials with `/api/agents/{id}/credentials` and `/api/agent-credentials/{id}/revoke`. Provider launchers use separate Run-bound runtime credentials and revoke them when the launch attempt finishes. Set `MORROWS_REQUIRE_AGENT_AUTH=1` to reject the legacy identity-header-only path even on loopback.

The MCP surface intentionally does **not** expose agent/profile/account/machine registration, fleet state, capacity, dispatch policy, dispatch, assignment claiming, Run creation, launch profile management, launch/cancel operations, or dependency graph administration. Those remain control-plane responsibilities through REST/store/provider adapters.

## Control-plane Operator auth

Control-plane REST supports issued `mrw_operator_*` Bearer credentials. `viewer` can read control-plane state; `operator` can mutate ordinary work/dispatch/launch state; `admin` additionally manages identities and Agent/Operator credentials. Bootstrap access uses `MORROWS_BOOTSTRAP_OPERATOR_TOKEN` as temporary admin authority; after issuing a durable admin credential, remove the bootstrap token from the environment and restart.

Credential management endpoints are `GET/POST /api/operator-credentials` and `POST /api/operator-credentials/{id}/revoke`. Only hashes are stored in SQLite and list responses never return token/hash material. The WebUI top bar can store one Operator token in browser local storage and attaches it to `/api/*` control-plane requests.

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

Direct conversations are separate from Task collaboration. The WebUI loads only conversation summaries at startup; selecting a conversation loads the latest message page into an in-memory cache, older history is fetched explicitly, and only the selected conversation polls for new messages. Human messages are durable and remain awaiting reply until the addressed Agent replies. A separate transactional `AgentDelivery` tracks whether each message has reached an Agent runtime. For one-turn local providers, queued delivery automatically resumes the same Run/provider thread after the current turn exits; Morrows does not create a second Run or LSM Logical Session solely for chat.

External adapters such as `lsm_external`, `antigravity_external`, `gemini_external`, and `codebuddy_external` remain compatibility launch backends. Their lifecycle endpoints are control-plane REST operations; they are no longer exposed as employee MCP tools. Provider-specific active launch adapters should be preferred when an automation API/CLI exists.

`MORROWS_*` settings take precedence; the former `AC_*` settings remain accepted during migration.

`X-Agent-Instance-Id` remains an identity binding hint, not a secret. Issued Agent and Operator Bearer credentials are stored only as SHA-256 hashes, and the two credential classes cannot cross their authorization planes. Morrows still refuses direct non-loopback plaintext HTTP by default even when both auth systems are strict, because Bearer credentials require transport security. For remote use, keep Morrows on loopback and expose it through a TLS reverse proxy; `MORROWS_ALLOW_INSECURE_REMOTE_HTTP=1` exists only for isolated development.

## Three-Plane Architecture & Core Invariant

Morrows cleanly decouples into three planes:
1. **Morrows Control Plane**: Source of truth for work semantics, assignments, context snapshots, long-term memory, decisions, and artifacts.
2. **LSM Runtime Plane**: Source of truth for host execution resources, isolated runtime scopes, jobs, shells, and execution audit.
3. **Provider Plane**: Source of truth for model-private rollout sessions, tool representations, and reasoning traces. Provider sessions are private and never directly shared.

> **Ultimate Recovery Invariant (终极恢复原则)**:
> Task identity is independent of model, account, machine, and conversation/session. Even if the original Agent process, Provider Session, execution machine, and temporary workspace directories disappear completely, a new Agent can reconstruct full state and proceed solely from Morrows canonical records.

### Normalized Naming Model (去歧义术语对照)

To avoid ambiguity from bare words like `session` and `run`, Morrows adopts a collaborative, Feishu-inspired user layer and precise system-layer definitions:

| 用户层 / 沟通词汇 | 系统层正式英文 | 历史代码别名 | 核心定义与语义范围 |
| :--- | :--- | :--- | :--- |
| **工作项** | `WorkItem` | `Task` | 组织内需要完成的一件客观工作任务，具独立生命周期 |
| **工作分配** | `WorkAssignment` | `Assignment` | 某个 Agent 实例对工作项中某特定角色的有期限责任（带租约 Lease） |
| **工作执行**（前端：执行记录） | `WorkExecution` | `Run` | 某个 Agent 对一次工作分配的具体逻辑执行过程 |
| **启动记录 / 启动尝试** | `AgentLaunch` | `LaunchAttempt` | 系统为推进工作执行，实际启动一次具体 Agent 进程的记录 |
| **运行空间** | `RuntimeScope` | LSM `Logical Session` | LSM 运行时为一次工作执行划定的安全隔离执行范围与权限作用域 |
| **执行任务** | `RuntimeJob` | LSM `Job` | 在运行空间内异步执行的具体机器任务（如 `cargo test`） |
| **持久终端 / 终端** | `PersistentShell` | `shell session` | 运行空间内维持环境与状态的常驻命令行交互终端 |
| **浏览器实例** | `BrowserInstance` | `browser session` | 运行空间内受控的有状态浏览器实例 |
| **模型会话** | `ProviderThread` | `Provider Session` | Codex / Claude / Gemini 模型自身的连续私有对话流 |
| **工作对话** | `DirectConversation` | `Conversation` | 人类与特定 Agent 实例直接进行的一对一持久化双向沟通 |
| **上下文快照** | `ContextSnapshot` | `ContextRevision` | 某次工作执行初始化时冻结的完整工作背景、目标与记忆视图 |
| **记忆** | `Memory` | `Memory` / `Context` | 长期持久化知识（跨越任务与会话），分组织/项目/员工/工作等作用域 |
| **成果物** | `Artifact` | `Artifact` | 被正式归档、持久托管并可全局引用的工作交付物证据（Managed Store） |
| **决策记录** | `Decision` | `Decision` | 经确认的、对后续工作产生约束与指导的技术或业务决断 |
| **工作交接** | `Handoff` | `Handoff` | 工作责任从一个执行转交至另一个执行的结构化交接协议 |
| **执行证据** | `WorkExecutionEvidence` | `RunExecutionEvidence`| 将工作语义状态与底层 LSM Audit、Job Logs 关联的追溯证据 |

> 完整规范请参阅：[docs/agent-work-and-context-spec.md](file:///Users/huayuxue/workspaces/morrows/docs/agent-work-and-context-spec.md) 与 [docs/architecture.md](file:///Users/huayuxue/workspaces/morrows/docs/architecture.md)

## Live handoff validation

M2.1 was exercised with two separate `codex-personal` CLI sessions and two distinct AgentInstances. Agent A created a typed artifact, decision, directed message, and pending handoff, which released A's assignment and ended A's Run as handed off. After the control plane assigned Agent B and created its Run, B reconstructed the state through employee MCP only, accepted the handoff, replied in the original thread, checkpointed the recovered state, and completed the task. No Codex session/chat history was shared between A and B.
