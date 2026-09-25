# Architecture

## Three-Plane Architecture (三平面架构)

Morrows separates the system into three decoupled planes with explicit boundaries of truth:

```
┌─────────────────────────────────────────────────────────────────────────┐
│ 1. Morrows Control Plane (控制平面)                                      │
│    工作语义与持久知识的 Source of Truth                                    │
│    负责：工作项、工作分配、工作执行生命周期、上下文快照、长期记忆、成果物托管、       │
│          决策记录、工作交接、工作对话、调度派发、执行证据关联等                │
└────────────────────────────────────┬────────────────────────────────────┘
                                     │
                     /api/control    │ (trusted loopback HTTP)
                     scoped token    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ 2. LSM Runtime Plane (运行平面)                                         │
│    执行证据与运行资源的 Source of Truth                                     │
│    负责：运行空间 (RuntimeScope)、执行任务 (RuntimeJob)、持久终端 (Shell)、    │
│          浏览器实例、文件系统/远程机器操作、底层审计日志 (Audit) 等             │
└────────────────────────────────────┬────────────────────────────────────┘
                                     │
                 scoped capability   │ (confines agent execution)
                                     ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ 3. Provider Plane (模型平面)                                            │
│    模型推理与私有交互会话的 Source of Truth                                  │
│    负责：Codex / Claude / Gemini / CodeBuddy 的模型上下文、Rollout /     │
│          私有对话流、内部思考过程 (Reasoning)、断点恢复 (Resume State) 等    │
│    * Provider 会话属于外部私有数据，不作为 Morrows 标准状态共享                  │
└─────────────────────────────────────────────────────────────────────────┘
```

## Terminology Normalization & Naming Model (去歧义术语规范)

To prevent severe ambiguity caused by bare usages of common words like `session` and `run`, Morrows defines a two-layer naming model:
1. **User Layer (用户层 / 交互心智模型)**: Aligned with collaborative organizational concepts (referencing Feishu / Lark product conventions);
2. **System Layer (系统层 / 规范对象)**: Strict, typed domain entities in store and API.

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
| **会话** | `Session` | `Conversation` | 人类与特定 Agent 实例之间的一对一持久化工作会话；属于一个 AgentInstance，可跨多次运行时 turn 持续存在 |
| **上下文快照** | `ContextSnapshot` | `ContextRevision` | 某次工作执行初始化时冻结的完整工作背景、目标与记忆视图 |
| **记忆** | `Memory` | `Memory` / `Context` | 长期持久化知识（跨越任务与会话），分组织/项目/员工/工作等作用域 |
| **成果物** | `Artifact` | `Artifact` | 被正式归档、持久托管并可全局引用的工作交付物证据（Managed Store） |
| **决策记录** | `Decision` | `Decision` | 经确认的、对后续工作产生约束与指导的技术或业务决断 |
| **工作交接** | `Handoff` | `Handoff` | 工作责任从一个执行转交至另一个执行的结构化交接协议 |
| **执行证据** | `WorkExecutionEvidence` | `RunExecutionEvidence`| 将工作语义状态与底层 LSM Audit、Job Logs 关联的追溯证据 |

> 规范详见：[docs/agent-work-and-context-spec.md](file:///Users/huayuxue/workspaces/morrows/docs/agent-work-and-context-spec.md)

### Agent UI naming rules

The UI must distinguish **human display names** from **canonical internal identifiers**:

- `AgentInstance.name` is a stable runtime/registration identity and MUST NOT be renamed by
  ordinary UI actions. `AgentInstance.display_name` is the human-facing Agent name.
- New normalized instances receive an automatic display name derived from their product/profile
  family plus a numeric suffix: `codex-0`, `codex-1`, `codebuddy-0`, `chatgpt-0`, etc.
  The suffix only disambiguates Agents of the same family; machine/account/session names do not
  belong in the display name.
- Users may rename `display_name`. Renaming MUST NOT change the runtime registration key,
  provider thread identity, account binding, machine binding, or historical references.
- `AgentProfile` is rendered as **类型 / Type**; `AgentAccount` as **身份 / Identity**;
  `Machine` as **运行位置 / Runtime location**; capacity state as **工作状态 / Work state**.
- An account is identified to users by its verified **email** when available. Internal labels
  such as `codex-personal` or `codebuddy-personal` are technical identifiers and stay in
  Technical info. If the provider does not expose an email, the UI shows **未记录邮箱 /
  Email not recorded** rather than inventing an identity.
- Manually provisioned Codex Agents are created by selecting an existing Codex `auth.json`
  through the Web UI's native file picker. Morrows derives the Account email from the
  `tokens.id_token` JWT claim, rejects missing/unverified email identities, and copies the
  selected auth file into one private `CODEX_HOME` per Account with
  `cli_auth_credentials_store="file"`. The Account record stores only the isolation
  backend/path metadata; provider secrets remain outside the Morrows database. This is an
  implementation boundary, not the long-term credential model.
- A future provider-credential layer should replace the per-home backend behind Account with a
  centralized encrypted Credential Store / broker. AgentInstance, Account, Session and
  LaunchProfile identities must remain stable during that migration so historical work does not
  depend on where authentication material is physically stored.
- Provider names are normalized for display (`openai → OpenAI`, `tencent → Tencent`,
  `local → 本地/Local`). UUIDs, raw instance names, external refs, account labels and
  profile/account/machine IDs remain available under the collapsed **技术信息 / Technical info**
  section for debugging and control-plane operations.
- Archived Agents remain durable identities for historical Tasks/Runs/messages but are omitted
  from the default Agent Fleet. Archiving also closes their open Sessions; it never
  deletes historical Session/messages.

## Knowledge, Memory & Summary Hierarchy (知识与记忆层级)

Knowledge is partitioned into four distinct tiers:
1. **Long-term Memory (长期记忆)**: Partitioned into `organization`, `project`, `agent`, and `task` scopes. Evolves via append-only revisions (`supersedes`). Raw tool outputs, excessive shell logs, and model conjectures MUST NOT enter long-term memory.
2. **ContextSnapshot (上下文快照)**: Immutable materialized view pinned to a `WorkExecution`. Answers: *"What exactly did this Agent know at execution time?"*
3. **Structured Summary (结构化摘要)**: Derived data composed of **Deterministic Facts** (changed files, git commits, tests run and exit codes, produced artifacts) + **Semantic Extraction** (goals, findings, blockers, next steps). Crucial findings MUST attach evidence references (`evidence_refs`).
4. **Context Package (上下文包)**: The standardized package for transferring state across Agents and Providers without sharing private chat history.

## Local architecture

```
┌──────────────────┐   ┌──────────────┐       ┌─────────────┐
│ Control clients  │──▶│ morrows-server│──────▶│ morrows-store│
│ Web UI / REST    │   │ Axum + rmcp  │       │ SQLx/SQLite │
└──────────────────┘   └──────┬───────┘       └─────────────┘
                              │ employee MCP
                              ▼
                         Managed agents
```

REST is the company control plane: registry, fleet, dispatch, Assignment/Run lifecycle,
launch, cancellation, and provider adapters. MCP is an employee-facing interface backed
by the same store/domain invariants, but exposes only work access, memory, collaboration,
handoff, and progress/completion reporting. MCP is not an HTTP-to-HTTP adapter.

### Agent credential boundary

Agent/bridge authentication is separate from operator/control-plane authentication.

Morrows can issue two credential classes for one `AgentInstance`:

- `runtime`: short-lived and bound to a `Run`; launch adapters mint one for each
  Codex/CodeBuddy process and `finish_launch_attempt` revokes active runtime
  credentials for that Run in the same transaction as process termination.
- `bridge`: explicitly issued/revoked by the trusted local control plane for an
  external provider bridge.

Only SHA-256 token hashes are stored in SQLite. The plaintext `mrw_agent_*` token is
returned once at issuance or held transiently by the launcher. HTTP authentication uses
`Authorization: Bearer ...`; the token subject becomes the canonical
`X-Agent-Instance-Id`. If that identity header is also supplied, it must match the token.

`MORROWS_REQUIRE_AGENT_AUTH=1` disables the legacy identity-header-only employee path.
The default loopback mode keeps that path temporarily for old local bridges.

### Operator/control-plane credential boundary

Control-plane REST uses a separate `mrw_operator_*` credential class. Durable credentials
carry one of three roles:

- `viewer`: read-only control-plane access;
- `operator`: ordinary work, dispatch, launch, cancellation, context and Session
  mutations;
- `admin`: operator privileges plus identity registration and Agent/Operator credential
  management.

`MORROWS_REQUIRE_OPERATOR_AUTH=1` closes the loopback no-auth compatibility path.
`MORROWS_BOOTSTRAP_OPERATOR_TOKEN` is an ephemeral admin authority for creating the first
durable admin credential; it should be removed from the environment after bootstrap.
Operator token hashes, like Agent credential hashes, are the only token material stored
in SQLite.

Agent and Operator credentials are deliberately non-interchangeable. Agent credentials
may access employee MCP and the small bridge/heartbeat REST surface, but are rejected by
control-plane REST. Operator credentials do not authenticate employee MCP or Agent
delivery endpoints.

Authentication is still not transport encryption. Direct non-loopback plaintext HTTP is
blocked by default even when both strict auth modes are enabled. Normal remote deployment
should keep Morrows bound to loopback and place a TLS reverse proxy in front of it.
`MORROWS_ALLOW_INSECURE_REMOTE_HTTP=1` is only an explicit isolated-development override.

## Implemented entities

```
Project
 └── MemoryEntry*

Task
 ├── MemoryEntry*
 ├── ContextRevision*
 ├── Assignment*
 │     └── Run*
 ├── Handoff*
 ├── Artifact*
 ├── Decision*
 ├── MessageThread*
 │     └── Message*
 ├── TaskDependency*
 └── Event*

Session
 └── SessionMessage*

Organization / AgentInstance
 └── MemoryEntry*
```

`MemoryEntry` is append-only long-term knowledge with an explicit scope
(`organization`, `project`, `agent`, or `task`), structured JSON content,
provenance (`source_kind` / `source_ref`), visibility, and an optional
`supersedes_memory_id` chain. It is distinct from mutable task progress and immutable
ContextRevision snapshots. Employee `memory_get` materializes the shared organization,
project, task, and caller-Agent memories relevant to the requested Task before returning
the current task context/collaboration state.

AgentInstance remains the worker identity referenced by M1/M2 work. M3 links each instance to an AgentProfile and optional independent Account and Machine identities, with append-only CapacitySnapshots.

## Important invariants

1. `claimed` is not a Task state.
2. Active ownership is represented by Assignment + lease.
3. One active Assignment exists per `(task, role)`.
4. Concurrent claims are resolved atomically by SQLite.
5. Run mutations require the same AgentInstance that owns the Assignment.
6. Context revisions are append-only/versioned.
7. Important actions append Events in the same transaction as state changes.
8. A successful `executor` Run may mark its Task done; non-executor roles do not.
9. Daemon restart does not erase Tasks, Runs, checkpoints, or pending durable jobs.
10. Session list reads return summary metadata only; message history is a separate paged read.
11. Session MCP access is scoped to the addressed AgentInstance.
12. **Ultimate Recovery Invariant (终极恢复原则)**: Even if the original Agent process, Provider Session, execution machine, and temporary workspace directories disappear completely, a new Agent can reconstruct full state and proceed solely from Morrows canonical records (WorkItem, ContextSnapshot, Memory, Decision, Artifact, Summary, Handoff, Evidence).

## Agent Sessions

Direct human↔Agent chat is intentionally separate from Task collaboration. A
`Session` belongs to one AgentInstance and stores only Session metadata; its
`SessionMessage` rows are paged independently. This keeps the WebUI startup path
lightweight and prevents opening the application from materializing every chat history.

Control-plane REST:

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/sessions` | Summary list only: title, Agent, counts, last-message preview/time |
| POST | `/sessions` | Create a Session for an AgentInstance |
| GET | `/sessions/{id}` | Session metadata |
| GET | `/sessions/{id}/messages?limit=&before=&after=` | Lazy/paged history; before loads older, after loads newer |
| POST | `/sessions/{id}/messages` | Queue a human message |
| POST | `/sessions/{id}/messages/{message_id}/recall` | Recall a human message while its delivery is still queued |
| GET | `/sessions/{id}/runtime` | Read the latest explicit Session runtime attempt |
| POST | `/sessions/{id}/runtime/start` | Explicitly start/resume the Session's local Agent CLI |

The WebUI keeps an in-memory cache keyed by Session ID. Initial navigation fetches
only summaries. Selecting a Session fetches the latest page (currently 80 messages);
switching away and back reuses the cache, the selected Session alone polls with an
`after` anchor, and earlier history is loaded only when requested with `before`.

Employee MCP exposes `session_inbox`, `session_get`, and
`session_reply`. Inbox returns only Sessions that still have queued human
messages for the authenticated AgentInstance. `session_get` enforces the target
Agent identity and supports the same paging anchors. A reply atomically marks queued
human messages in that Session delivered and appends the Agent reply.

Session messaging is durable but does not require an always-running Agent. A human
message first enters the durable delivery outbox. The user-facing delivery states are
only **等待投递 / awaiting delivery** and **已投递 / delivered**. Internal claim
state is deliberately not a third user-facing state. A message becomes delivered only
after Morrows has successfully written the Session prompt containing that message to
the target Agent CLI/runtime. While the delivery is still truly queued, the human may
recall it; once an Agent runtime has claimed it, recall is fenced even though the UI
continues to show awaiting delivery until the prompt write succeeds.

The WebUI can explicitly start the Agent for a Session without creating a fake Task,
Assignment, or Run. A `SessionRuntimeAttempt` binds the Morrows Session, AgentInstance,
the Agent's configured Account, and one enabled local CLI LaunchProfile. Each attempt can
also override `model` and `reasoning_effort` without mutating the LaunchProfile. The WebUI
loads these choices from the installed CLI: Codex uses its bundled machine-readable model
catalog, while CodeBuddy uses the model/effort values declared by its current CLI help. If a
later runtime omits either override, Morrows inherits the previous Session runtime setting. Morrows exports
the Agent, Account, and Session identifiers to the child environment, issues a short-lived
Session-scoped Agent credential, and claims only queued messages belonging to that
Session. For Codex/CodeBuddy local CLI adapters, the first successful start creates a
provider thread/session; later starts for the same Morrows Session and LaunchProfile
resume the latest persisted provider session reference. The runtime attempt records PID,
logs, exit status, and provider session reference.

If no explicit Session runtime is started, queued messages can still be consumed when
the addressed Agent later runs and reads its Session inbox; existing interrupted
task-bound local Runs may also be resumed by the delivery worker. Daemon restart marks
orphaned Session runtimes failed, revokes their Session credentials, and releases
unfinished delivery claims back to the queue.

## Collaboration continuation

M2 adds Handoff, Artifact, Decision, MessageThread/Message, and TaskDependency, then validates:

```
Agent A → checkpoint → handoff → Agent B → continue
```

without sharing the original chat session.

## M2 collaboration

M2 adds append-only Artifact (title, URI, description, kind defaulting to `other`), Decision (title and rationale),
MessageThread/Message, and structured Handoff records. Artifact URIs are references;
the daemon does not fetch or execute their contents. TaskDependency edges point from
work to its prerequisite. Self-links, duplicates, and cycles are rejected. Executor
claims and completion require all prerequisites to be done; other roles can collaborate
while prerequisites are unfinished. Dependency removal is explicit and audited.

A handoff contains summary, completed work, remaining work, blockers, and artifact/decision
references. It pins the current immutable ContextRevision and requires at least one remaining
work item. Creating it requires a live source run and an unexpired assignment owned by the
caller. In one SQLite write transaction, it validates task-scoped references, saves the handoff,
marks all live runs on the source assignment `handed_off`, releases that assignment, and
appends `handoff.created`. The task remains unfinished. The next agent reads the handoff,
claims the released role, starts its own run, and explicitly accepts the handoff. There is no dispatcher or agent launcher.

Run start, checkpoint, completion, renewal, and handoff serialize ownership checks with
writes. A released or expired session cannot subsequently complete the task. Context
revision creation and dependency cycle checks also serialize their read/write transactions.

REST additions (under `/api`):

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/tasks/{id}/collaboration` | All task collaboration records, including messages and prerequisites |
| POST | `/tasks/{id}/artifacts` | Create `{title, uri, description?, kind?}` |
| POST | `/tasks/{id}/decisions` | Create `{title, rationale}` |
| POST | `/tasks/{id}/threads` | Create `{title}` |
| POST | `/threads/{id}/messages` | Append `{body, message_type?, recipient_agent_instance_id?, recipient_role?, reply_to_message_id?, correlation_id?, requires_response?, status?}` |
| POST | `/runs/{id}/handoffs` | Create `{summary, completed, remaining, blockers?, artifact_ids?, decision_ids?}` |
| POST | `/handoffs/{id}/accept` | Accept with `{target_run_id}` |
| GET | `/handoffs/{id}` | Read handoff, pinned context, and referenced artifacts/decisions |
| POST | `/tasks/{id}/dependencies` | Add `{depends_on_task_id}` |
| DELETE | `/tasks/{id}/dependencies/{dependency}` | Remove prerequisite |

All new REST mutations require `X-Agent-Instance-Id` identifying a registered instance,
matching the existing MCP identity convention. This local identity header is not a new
credential/authentication system. M1 REST payload conventions remain compatible.

The employee MCP exposes collaboration tools such as `artifact_create`, `decision_create`,
`thread_create`, `message_create`, `handoff_create`, `handoff_get`, `handoff_accept`,
and `task_collaboration`. It does not expose dependency graph administration, claiming,
or Run creation. Task-scoped operations require `X-Agent-Instance-Id` and verify that the
caller owns or has been assigned the Task. The Task Detail UI displays all collaboration
entities.

Continuation sequence:

1. The control plane assigns Agent A and creates its Run. A reads `task_get` /
   `memory_get`, then reports checkpoints.
2. A creates artifacts/decisions and calls `handoff_create` with remaining work.
3. The control plane assigns Agent B and creates B's Run. B calls `task_collaboration`,
   then `handoff_get` to recover durable state.
4. B accepts with `handoff_accept(handoff_id, target_run_id)` and completes the remaining work.

Integration tests cover this sequence across database reopen, rollback on invalid references,
concurrent handoffs and reverse dependency edges, stale ownership, dependency gating,
non-executor completion, and REST/MCP adapter behavior. Existing M1 tests remain in place.

M2.1 adds optional directed recipients (registered agent instance and/or role), same-thread
reply links, and correlation IDs to messages. Message type defaults to `note`, status to
`sent`, and requires_response to false; these describe the stored message and do not
automatically dispatch work or mark requests answered. Type/status/kind are nonempty
string labels. Replies must reference an existing message in the same thread.

Handoffs start `pending` with no accepting run. Acceptance changes status to `accepted`
and sets `accepted_by_run_id` in the same transaction as `handoff.accepted`. It requires
a running or paused target run on the same task, owned by the caller, with an active,
unexpired assignment. Duplicate or concurrent acceptance has exactly one winner.
The REST body uses `target_run_id`; MCP takes `handoff_id` and `target_run_id`.
The additive 0003 migration preserves existing M2 records with the defaults above.

## Live CLI validation

M2.1 was validated against the persistent local daemon with two independent `codex-personal`
CLI sessions bound to two different AgentInstance identities. Agent A created collaboration
records and a pending handoff, ending its Run and releasing its Assignment. Agent B had no
access to A's provider thread; after the control plane assigned it a distinct Run, it
recovered the task via `task_get`, `memory_get`, `task_collaboration`, and `handoff_get`,
accepted the handoff, replied to A's directed message with the same correlation ID, checkpointed
the reconstructed state, and completed the task. The persisted handoff's
`accepted_by_run_id` points to B's Run, and the reply's `reply_to_message_id` points to A's
request.

## M3 identity and capacity

An **AgentProfile** describes a stable product/template (`name`, `provider`, `kind`,
`default_capabilities`, `metadata`). An **Account** describes an independent provider
identity (`provider`, `label`, optional `external_account_ref`, `status`, `metadata`).
An account can be used with multiple profiles from the same provider. A **Machine**
describes a host (`name`, `hostname`, `os`, `arch`, `status`, `metadata`, `last_seen_at`).
All three have UUIDs and `created_at`/`updated_at` timestamps.

Registration creates or returns the existing row unchanged by these natural keys:
profile `(provider, name)`, account `(provider, label)`, machine `name`. Name/label must
be nonempty. Optional metadata defaults to `{}`, profile capabilities to `[]`, account
and machine status to `active`; unspecified machine descriptors and profile kind are
empty strings. No credentials or secrets are stored or requested.

**AgentInstance** keeps its original UUID, name, status, capabilities and heartbeat,
adding required `profile_id`, optional `account_id`, `machine_id`,
`external_instance_ref`, and `created_at`. Registration requires existing foreign keys;
nonempty account/profile providers must match exactly. Omitted capabilities inherit
profile defaults; explicit `[]` means no capabilities. The existing unique name remains
the registration key. Repeating normalized registration with the same links reuses the
UUID and refreshes status/capabilities/heartbeat; different links for an existing name
return a conflict rather than repointing the identity of historical work.

Migration `0004_identity_capacity.sql` adds columns in place and does not replace or
renumber instances, assignments, runs, messages, or handoffs. It backfills one explicit
`legacy` profile and per-instance legacy account/unknown-machine rows. Backfilled
account/machine UUIDs equal the corresponding instance UUID in their separate tables;
this makes migration deterministic without guessing shared real accounts or hosts.
The original heartbeat is used for backfilled creation time because M1 did not store
instance creation time. SQLite triggers enforce the required profile/time after
backfill, avoiding a rebuild of the heavily referenced instance table.

Legacy `agent_register(name, capabilities)` and `POST /api/agents` still return
AgentInstance. New legacy callers create/reuse the legacy profile/account and create an
explicit unknown host; repeat calls reuse the same instance and links. Refreshing a
normalized instance through legacy registration preserves its normalized links. Existing
`X-Agent-Instance-Id` configuration, M1 REST payload conventions, leases, run ownership,
and M2/M2.1 handoff/message semantics remain unchanged.

Heartbeat requires `X-Agent-Instance-Id` to equal the target instance UUID. In one
transaction it sets instance status and server heartbeat time, advances the linked
machine's `last_seen_at` without moving it backwards, and optionally appends capacity.
Invalid optional capacity leaves the entire heartbeat unchanged. Heartbeats do not
renew assignment leases or change runs, and status is an opaque nonempty label; there
is no automatic stale/offline classification or dispatch.

A **CapacitySnapshot** contains UUID, instance UUID, status, `available_slots`,
`active_assignments`, `active_runs`, optional `max_concurrency`, optional opaque string
`quota_state`, `details` (default `{}`), and server `observed_at`. Counts are reported
observations, not derived scheduling authority. All counts must be nonnegative and
available slots cannot exceed a supplied maximum. Capacity recording uses the same
ownership check as heartbeat but does not itself refresh heartbeat/status. Snapshots
cannot be updated or deleted, enforced in SQLite as well as the store API. History is
newest observation first, with insertion order breaking timestamp ties; latest is
`null` for a registered instance with no observations, and an unknown instance is an
error. No quota parsing, billing adapters, launcher, remote auth, or Dispatcher is added.

REST routes (under `/api`):

| Method | Path | Body / result |
| --- | --- | --- |
| POST / GET | `/agent-profiles` | Register profile / list profiles |
| GET | `/agent-profiles/{id}` | Profile |
| POST / GET | `/accounts` | Register account / list accounts |
| GET | `/accounts/{id}` | Account |
| POST / GET | `/machines` | Register machine / list machines |
| GET | `/machines/{id}` | Machine |
| POST / GET | `/agent-instances` | `{name, profile_id, account_id?, machine_id?, capabilities?, external_instance_ref?}` / list instances |
| GET | `/agent-instances/{id}` | Instance |
| POST | `/agent-instances/{id}/heartbeat` | `{status, capacity?}`; returns instance |
| POST / GET | `/agent-instances/{id}/capacity` | Capacity fields / full newest-first history |
| GET | `/agent-instances/{id}/capacity/latest` | Snapshot or `null` |
| GET | `/agent-fleet` | `[{instance, profile, account, machine, latest_capacity}]` |

Only heartbeat and capacity mutations above require the identity header; identity
registration remains available before a caller has an instance ID. Missing/invalid
headers and invalid counts return 400; cross-instance mutation returns 409; missing
identities return 404. Nullable account/machine links appear as `null` in fleet results.

Agent/profile/account/machine registration, heartbeat, capacity, and fleet inspection are
control-plane operations exposed through REST and the Web UI. They are intentionally absent
from employee MCP. A provider worker or Morrows-owned bridge reports heartbeat/capacity on
behalf of the managed runtime; an employee agent cannot register or advertise itself through MCP.

The Fleet UI polls the joined endpoint and shows profile, account, machine, instance
status, capabilities, heartbeat age, and latest capacity with its own observation age.
Missing capacity is shown explicitly rather than inferred as zero. M3 tests upgrade a
seeded M2.1 database using production migrations, compare all existing collaboration
rows, exercise the original run owner after migration, verify snapshot durability and
append-only constraints, and cover registration/ownership/validation through REST/MCP.

## M4 Dispatcher

M4 adds a durable, explainable assignment Dispatcher on top of the M3 identity and
capacity model. It does not launch, resume, or control external agent processes.

A `TaskDispatchPolicy` is keyed by task and role. It stores required capabilities,
optional AgentProfile / Account / Machine filters, heartbeat and capacity freshness
TTLs, assignment lease duration, and an enabled flag. Updating a policy is audited;
manual `task_claim` remains supported and unchanged.

`dispatch_preview` and actual dispatch use the same evaluator. For an executor task,
the evaluator requires a dispatchable task state, completed prerequisites, no live
assignment for that role, and an enabled policy. A candidate must be online, satisfy
all capability and optional identity filters, and have both a fresh heartbeat and a
fresh capacity observation. Capacity statuses `blocked`, `throttled`,
`unavailable`, or `offline` are rejected; quota states `usage_limited`,
`exhausted`, or `blocked` are also rejected.

CapacitySnapshot remains observation-only. To avoid oversubscribing from a stale but
still fresh snapshot, Dispatcher reconciles the reported values with current durable
Assignments:

`added_since_snapshot = max(current_active_assignments - observed_active_assignments, 0)`

`effective_slots = max(reported_available_slots - added_since_snapshot, 0)`

When `max_concurrency` is present, effective slots are additionally capped by
`max_concurrency - current_active_assignments`. Candidate order is deterministic:
highest effective slots, then fewest current active assignments, then freshest
capacity observation, then AgentInstance UUID.

`dispatch_task` and `dispatch_next` execute stale-lease cleanup, re-evaluation,
selection, Assignment creation, and audit writes inside SQLite `BEGIN IMMEDIATE`.
This serializes concurrent dispatch attempts so two callers cannot consume the same
last effective slot. Assignment expiry discovered by Dispatcher emits the same
`assignment.expired` audit event shape as the lease manager.

Each mutating attempt appends a `DispatchDecision` containing the full preview,
candidate reasons, outcome, selected instance when applicable, and resulting
Assignment. Decisions are append-only at the SQLite layer. Preview is read-only.
`dispatch_next` considers enabled policies by task priority descending, then task
creation time and UUID, recording no-candidate attempts until the first assignment.

REST additions under `/api`:

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/dispatch-policies` | List durable policies |
| POST | `/tasks/{id}/dispatch-policy` | Create or replace a policy |
| GET | `/tasks/{id}/dispatch-policy/{role}` | Read a policy |
| GET | `/tasks/{id}/dispatch-preview/{role}` | Explain task/candidate eligibility without mutation |
| POST | `/tasks/{id}/dispatch/{role}` | Atomically select and create an Assignment |
| GET | `/tasks/{id}/dispatch-decisions` | Read append-only decision history |
| POST | `/dispatch/next/{role}` | Dispatch the highest-priority eligible task |

Dispatcher policy, preview, dispatch, and decision history are control-plane REST/Web UI
operations. They are intentionally absent from employee MCP. The resulting Assignment is
owned by the selected AgentInstance; employee MCP can only renew that existing Assignment
and report on its Run.

The Work Queue marks tasks with enabled executor policies. Task Detail can edit the
basic capability/lease policy, preview concrete candidate rejection reasons, dispatch
explicitly, and inspect recent decisions. M4 deliberately defers executor launch
adapters, provider-specific quota parsers, preemption, remote authentication/RBAC, and
a periodic automatic dispatch loop.

## M5 Executor launcher

M5 consumes an existing executor Assignment without changing Dispatcher policy. An
operator registers a `LaunchProfile` for that AgentInstance. A local `codex_cli`
profile fixes the absolute executable and workspace; task text cannot choose the
program, cwd, environment, or argv. The bounded prompt is written to stdin.

`launch_enqueue` checks the live Assignment and profile in one immediate transaction.
For Codex, it creates a queued LaunchAttempt and a durable launch job. The worker
creates the Run before spawn, then starts either a new session:

`<program> exec --json --color never --approve-for-me -C <cwd> -o <last-message> [ -m <model> ] -`

or resumes a previous finished attempt for the same task, agent, and profile:

`<program> exec resume --json -o <last-message> [ -m <model> ] <session-id> -`

The worker records JSONL, stderr, last message, PID, and external session reference
under `MORROWS_LAUNCH_DIR/<attempt-id>/`. A `launch_stop` request atomically revokes
the assignment and marks the attempt cancelled; the owning worker polls that state
and kills its child. On restart, the daemon marks interrupted local attempts failed
and restores claimed jobs whose attempts had not started. It cannot safely identify
and kill an orphan process after an unclean OS crash.

`lsm_external`, `antigravity_external`, `gemini_external`, and `codebuddy_external`
profiles contain no executable or workspace. Enqueue writes an `awaiting_agent` attempt.
Listing/accepting those invitations is now a control-plane REST/provider-bridge
responsibility rather than an employee MCP operation. Once Morrows has created the
Assignment and Run, the agent uses `task_get`, `memory_get`, `instructions_get`,
checkpoints progress, and calls `run_complete` when done. These compatibility adapters
still do not open a product UI or start a provider process.

`launch_instruction_send` remains a control-plane mailbox operation. Employees read the
ordered task instruction history with `instructions_get`; on resume, launch adapters also
include prior instructions in the new prompt.

Process exit is distinct from semantic completion. `run_complete` wins if already
committed. A zero exit without it pauses the Run, releases the Assignment, and
returns the Task to `ready`; a failed process marks the Run failed and does the same.

REST additions under `/api`:

| Method | Path | Purpose |
| --- | --- | --- |
| GET/POST | `/launch-profiles` | List/register launch profiles |
| GET | `/launch-profiles/{id}` | Read a profile |
| POST | `/launch-attempts/enqueue` | Queue Codex or invite an external agent |
| GET | `/launch-attempts/{id}` | Read attempt status |
| POST | `/launch-attempts/{id}/stop` | Stop and release an attempt |
| GET/POST | `/launch-attempts/{id}/instructions` | Read/send instructions |
| POST | `/launch-attempts/external/accept` | Accept as assigned AgentInstance |
| GET | `/agent-instances/{id}/external-launches` | Read external work for an instance |
| GET | `/tasks/{id}/launch-attempts` | Task launch history |
| GET | `/tasks/{id}/launch-instructions` | Task instruction history |

Launch profiles, attempts, enqueue/stop, instruction sending, and external invitation
acceptance are control-plane REST/Web UI operations and are intentionally absent from
employee MCP. The Web Task Detail view supports launch/invite, resume, instruction send,
stop, and status. Agent/provider and operator/control-plane calls now use separate issued
Bearer credential classes. Interactive multi-user identity/SSO, TLS termination, and remote
process launch remain deployment milestones.
