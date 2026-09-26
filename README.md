# Morrows

**简体中文** | [English](README.en.md)

Morrows 是一个本地优先、Agent 原生的工作协作系统，用于协调人类与多个 AI Agent 产品、账号和运行环境。

当前的本地纵向切片明确区分“公司控制面”和“员工操作面”：

```text
人类 / Web UI / 自动化
          ↓ REST
   Morrows 控制平面
   ├─ 注册表 / Agent 团队
   ├─ 调度器
   ├─ 工作分配 / 执行生命周期
   └─ Provider 启动适配器
          ↓
        Agent
          ↓ 员工 MCP
   工作 / 记忆 / 协作 / 汇报
```

## 当前实现

- Rust + Tokio
- Axum HTTP 服务
- SQLite + SQLx migrations
- React + Vite Web UI；生产模式下由 Rust daemon 直接提供
- 基于 `rmcp 3.x` 的 MCP Streamable HTTP 服务
- 持久化 Task / Assignment / Run / ContextRevision / Event 记录
- 原子化并发任务领取
- Assignment lease 续期与自动过期扫描
- 在 store/domain 边界进行 Run 所有权校验
- 不可变 ContextRevision
- 按 organization / project / Agent / task 作用域保存的长期 `MemoryEntry`，支持来源追踪与 supersede 链
- 员工 [记忆文件 CLI](docs/memory-files.md)：多项目完整文本 checkout，原生 rg/grep/sed，带来源、历史和冲突检查的显式发布；[Git 版本管理设计](docs/memory-versioning.md) 尚未迁移上线
- 持久化 Artifact / Decision / MessageThread / Message / Handoff / TaskDependency
- 支持 reply/correlation metadata 的 Agent 定向消息
- 原子化 Handoff 创建，以及与接受方 Run 关联的显式接受流程
- 依赖环检测与 executor gating
- 规范化 AgentProfile / Account / Machine / AgentInstance 身份模型
- 受所有权约束的 heartbeat、append-only capacity 观测，以及聚合 Agent Fleet UI
- 持久化的逐任务调度策略与 append-only dispatch decision
- 可解释、容量感知的 Dispatcher，以及原子 Assignment 创建
- 持久化 executor LaunchProfile / LaunchAttempt 与后台 launch job
- 安全的 Codex CLI 启动、Session 恢复、停止，以及 Run/Session 对账
- 持久化 launch instruction，可在 Web UI 和员工 MCP 中查看
- 一等公民 Agent Session：列表只加载摘要，消息历史分页读取
- Session WebUI 缓存：只有选中 Session 后才拉取历史，并仅对当前 Session 增量刷新
- 基于持久化 Agent delivery outbox 的员工 Session inbox/read/reply MCP 工具
- 可签发/撤销的 Agent Bearer credential；runtime credential 短期且绑定 Run，bridge credential 由本地控制面显式签发
- 可签发/撤销的控制平面 Operator credential，支持 `viewer` / `operator` / `admin` RBAC；WebUI 可在本地保存 Operator token
- LSM、Antigravity、Gemini 等 external handoff adapter，以及受所有权约束的 accept/status
- 本地 launch job 中断后的启动恢复
- 可选 LSM Run 集成：每个 Run 一个持久 Logical Session、作用域化 Codex MCP 权限、独立 control API、执行证据、显式 restart 与有界 cleanup
- 中英文 Web UI，首次访问默认中文
- SQLite 持久化 launcher job 与支持 provider resume 的事务型 Agent delivery outbox
- Agent Fleet 可手动添加 Codex Agent：填写账号邮箱与目标机器上的凭据引用（如 `~/.codex`、`~/.codex-personal`）；Morrows 不上传、不复制、不保存 Codex `auth.json` 或 Provider 登录 token
- 每个受管 Codex Account 使用独立认证目录；认证文件不写入 Morrows 数据库，也不会提交到 Git
- 可复用的本地部署脚本 `scripts/deploy.sh`，负责构建、重启 tmux 服务并执行健康检查

尚未实现：所有外部 Agent 产品的直接进程控制、交互式多用户账号/SSO、内置 TLS 终止、分布式部署。当前 external adapter 会邀请已有 Agent Session，而不会自动打开对应产品。

## 运行

从仓库根目录开始。用于本地开发时仍可直接运行：

```bash
cd web
npm install
npm run build
cd ..

cargo run -p morrows-server
```

生产部署默认运行在 OVH VPS。提交并 push 后，从本机执行：

```bash
./scripts/deploy-vps.sh
```

该脚本会在 VPS 上同步 `main`、构建 Web UI 和 release 后端，并通过
`morrows.service` 重启服务。生产启动还会逐字比较该脚本保留的发布文件（不做 checksum 检查）：Git commit、
后端二进制、`web/dist`、`run-vps.sh` 和已安装的 systemd unit 必须全部一致。
手工 `git pull/reset`、手工 rebuild、替换前端或 unit 后直接重启都会以 exit 78 拒绝启动，并提示：

```text
ERROR: refusing to start an unmanaged Morrows production deployment.
Deploy with: ./scripts/deploy-vps.sh
```

正常重启一个未被修改的已部署版本仍然允许。Morrows 本身只监听 VPS loopback
`127.0.0.1:8787`；公网 MCP 通过现有 Cloudflare Tunnel 与 VPS 路由层暴露为：

```text
https://mcp.xycdev.com/morrows
```

WebUI 同一实例暴露在：

```text
https://mcp.xycdev.com/morrows/ui/
```

控制平面 API 在公网模式强制 Operator Bearer 认证；WebUI 顶栏可保存对应 token。

VPS 上的 Local Shell MCP 已移到 `127.0.0.1:8766`。本地路由层占用
`127.0.0.1:8765`：仅 `/morrows` 转发到 Morrows，其余路径继续转发给
Local Shell MCP，因此原有 `https://mcp.xycdev.com/mcp` 保持不变。这个共享
Caddy 入口由 `local-shell-mcp` 仓库的 `deploy/morrow/deploy-vps.sh` 统一管理；
Morrows 不再维护第二份路由配置。

本地开发仍可使用旧的 tmux 部署：

```bash
./scripts/deploy.sh
./scripts/deploy.sh --fast
```

本地开发地址仍为 `http://127.0.0.1:8787`。

环境变量：

```bash
MORROWS_DATABASE_URL=sqlite://data/morrows.db
MORROWS_BIND=127.0.0.1:8787
MORROWS_WEB_DIR=web/dist
MORROWS_LAUNCH_DIR=data/launches
MORROWS_MCP_URL=https://mcp.xycdev.com/morrows
# rmcp Host 校验会自动允许 MORROWS_MCP_URL 中的 hostname。
# 如需额外 Host，可用逗号分隔：
# MORROWS_MCP_ALLOWED_HOSTS=internal.example:9443

# 可选：loopback LSM 集成
MORROWS_LSM_CONTROL_URL=http://127.0.0.1:8766
MORROWS_LSM_SUBJECT=local-mcp-client
MORROWS_AGENT_RESTART_GRACE_SECONDS=600
# VPS 上无需复制 MORROWS_LSM_CONTROL_KEY：
# scripts/run-vps.sh 只从 LSM 的私密 service.env 读取
# LOCAL_SHELL_MCP_CONTROL_API_KEY，并在进程启动时映射。

# loopback 上可选；任何面向远程的部署都要求开启
MORROWS_REQUIRE_AGENT_AUTH=1
MORROWS_REQUIRE_OPERATOR_AUTH=1

# Bootstrap admin credential，仅用于签发第一个持久 admin token。
# 完成 bootstrap 后应从环境中移除并重启。
MORROWS_BOOTSTRAP_OPERATOR_TOKEN=mrw_operator_<secret>

# 默认禁止直接通过非 loopback 的明文 HTTP 暴露服务。
# 推荐 loopback + TLS reverse proxy。以下仅用于隔离开发环境：
MORROWS_ALLOW_INSECURE_REMOTE_HTTP=0
```

## 测试

```bash
cargo test --workspace
cd web && npm run build
```

或：

```bash
./scripts/check.sh
```

## MCP

Morrows 暴露一个 Streamable HTTP MCP endpoint，用于**员工操作**，而不是公司管理：

```text
http://127.0.0.1:8787/mcp
```

员工 MCP 调用支持签发的 Bearer credential：

```text
Authorization: Bearer mrw_agent_<secret>
X-Agent-Instance-Id: <uuid>   # 使用 Bearer 时可选；若提供则必须匹配
```

本地控制面可通过 `/api/agents/{id}/credentials` 和 `/api/agent-credentials/{id}/revoke` 签发、列出和撤销 bridge credential。Provider launcher 使用独立的、绑定 Run 的 runtime credential，并在 launch attempt 结束后撤销。设置 `MORROWS_REQUIRE_AGENT_AUTH=1` 后，即使在 loopback 上也会拒绝旧的仅 identity-header 模式。

MCP surface 有意**不提供** Agent/profile/account/machine 注册、fleet 状态、capacity、dispatch policy、dispatch、Assignment 领取、Run 创建、LaunchProfile 管理、launch/cancel 或 dependency graph 管理。这些仍属于控制平面的 REST/store/provider adapter 职责。

## 控制平面 Operator 认证

控制平面 REST 支持签发的 `mrw_operator_*` Bearer credential。`viewer` 可读取控制面状态；`operator` 可修改普通 work/dispatch/launch 状态；`admin` 还可以管理身份以及 Agent/Operator credential。

Bootstrap 阶段使用 `MORROWS_BOOTSTRAP_OPERATOR_TOKEN` 作为临时 admin 权限；签发持久 admin credential 后，应从环境中移除 bootstrap token 并重启。

Credential 管理 endpoint：

- `GET/POST /api/operator-credentials`
- `POST /api/operator-credentials/{id}/revoke`

SQLite 中只保存 hash，列表响应不会返回 token/hash 原文。WebUI 顶栏可在浏览器 local storage 中保存一个 Operator token，并自动附加到 `/api/*` 控制平面请求。

当前员工 MCP 工具包括：

- `session_inbox`、`session_get`、`session_reply`：接收并回复发给当前 Agent 的 Session。
- `work_request_submit`：提交新的工作请求，但不能自行选择优先级、负责人或 launcher。
- `whoami`：确认当前 Agent 身份与默认查询范围。
- `task_list`：默认列出分配给当前 Agent 的未完成任务；支持指定 Agent、项目、状态、分页，或用 `scope=all` 查询其他任务。
- `project_list` / `project_get`：获取项目摘要、完整背景和当前共享记忆。
- `task_get`：读取任意任务及分页的分配记录、调用者自身的执行记录。
- `task_context`：开工首选入口；实时组合任务、项目背景、当前上下文、记忆、关键协作和管理指令，显式标记缺失背景与结构化验收条件。
- `memory_get` / `memory_revise`：分页读取当前有效记忆与上下文，或在具备任务写入权限时修订。消息历史通过 `task_collaboration` 单独分页读取。
- `instructions_get`：分页读取任意任务的管理指令，只确认本页中发给调用者的投递。
- `artifact_create`、`decision_create`、`thread_create`、`message_create`：记录成果与协作信息。
- `handoff_create`、`handoff_get`、`handoff_accept`、`task_collaboration`：无需共享 Provider chat history 即可跨 Agent 延续工作。
- `assignment_renew`、`run_checkpoint`、`run_complete`、`task_events`：维护已有 Assignment 并汇报进度/完成状态。
- `task_request_assignment` / `assignment_request_list` / `assignment_request_withdraw`：申请接手现有任务、查询控制面处理结果或撤回待处理申请；不创建重复任务，也不自行认领或启动。
- `project_memory_publish`：参与任务的 Agent 直接写入所属项目知识，保留来源、验证边界与旧版本，支持幂等重试和旧版本冲突检查。
- `run_completion_check`：只读获取原始验收条件、报告模板和阻塞原因。声明了 `acceptance_criteria` / `freeze_requires` 的执行任务，完成时必须提供当前上下文和逐项证据引用；服务器不代替实际实验验证。

任务和项目读取要求已认证的 Agent 身份，不要求任务归属。默认按 Agent 筛选只是发现偏好，不是读取权限边界。协作写入仍要求拥有任务、分配历史或开放的任务会话；执行更新和私人会话仍校验归属。MCP 不能自行创建 Assignment 或 Run。完整参数、分页与返回格式见 [员工查询协议](docs/employee-discovery.md)。

Session 是独立的持久对话对象，但可以选择作用域：**通用会话**不绑定工作，**项目会话**绑定 Project，**任务会话**绑定 Task（其 Project 自动由 Task 推导）。WebUI 启动时只加载 Session 摘要；选中某个 Session 后，才将最新消息页载入内存缓存；更老历史需要显式加载，并且只有当前选中的 Session 会轮询新消息。

人类消息会持久保存，并在目标 Agent 回复前保持等待状态。独立的事务型 `AgentDelivery` 只暴露两个面向用户的投递状态：**等待投递**与**已投递**；“已投递”表示 Morrows 已成功把消息写入 Agent runtime prompt。消息在 runtime claim 前可以撤回。

WebUI 可以为某个 Session 显式启动/恢复本地 Agent CLI。专用 Session runtime 会绑定 AgentInstance、Account、LaunchProfile、Morrows Session 和持久化 Provider thread/session reference，而不会伪造 Task 或 Run。Task 启动 Agent 时也会自动绑定该 Task + Agent 的开放 Session（不存在则创建），并与直接 Session runtime 共享同一套 Provider thread 续接来源；Run / LaunchAttempt 只描述执行生命周期，不再承担独立的对话身份。通用会话的消息不会被 Task launch 误领取，也不会唤醒无关的中断 Run。

`lsm_external`、`antigravity_external`、`gemini_external`、`codebuddy_external` 等 external adapter 仍作为兼容 launch backend 存在。它们的生命周期 endpoint 属于控制平面 REST，不再暴露为员工 MCP 工具。当 Provider 存在自动化 API/CLI 时，应优先使用 Provider-specific active launch adapter。

`MORROWS_*` 配置优先；旧的 `AC_*` 设置在迁移阶段仍兼容。

`X-Agent-Instance-Id` 仍只是身份绑定提示，不是 secret。Agent 与 Operator Bearer credential 都只以 SHA-256 hash 形式存储，且两个 credential class 不能跨授权平面使用。

即使 Agent/Operator 认证都处于严格模式，Morrows 默认仍拒绝直接面向非 loopback 的明文 HTTP，因为 Bearer credential 需要传输层安全。远程使用时，应让 Morrows 继续监听 loopback，并通过 TLS reverse proxy 暴露；`MORROWS_ALLOW_INSECURE_REMOTE_HTTP=1` 只用于隔离开发环境。

## 三平面架构与核心恢复原则

Morrows 明确拆分为三个平面：

1. **Morrows Control Plane**：工作语义、工作分配、上下文快照、长期记忆、决策和成果物的事实来源。
2. **LSM Runtime Plane**：主机执行资源、隔离运行空间、Job、Shell 和执行审计的事实来源。
3. **Provider Plane**：模型私有 rollout Session、工具表示和 reasoning trace 的事实来源。Provider Session 属于私有状态，不直接在 Agent 之间共享。

> **终极恢复原则（Ultimate Recovery Invariant）**
>
> Task 身份独立于模型、账号、机器、Morrows Session 和 Provider Thread。即使原 Agent 进程、Provider Session、执行机器和临时 workspace 全部消失，新 Agent 仍应能仅依靠 Morrows 的 canonical records 重建完整状态并继续工作。

### 规范化命名模型

为避免裸用 `session`、`run` 等词造成歧义，Morrows 在用户层采用偏协作产品的中文命名，在系统层保留精确定义：

| 用户层 / 沟通词汇 | 系统层正式英文 | 历史代码别名 | 核心定义与语义范围 |
| :--- | :--- | :--- | :--- |
| **工作项** | `WorkItem` | `Task` | 组织内需要完成的一件客观工作，具有独立生命周期 |
| **工作分配** | `WorkAssignment` | `Assignment` | 某个 Agent 实例对工作项中某特定角色的有期限责任，带 Lease |
| **工作执行**（前端：执行记录） | `WorkExecution` | `Run` | 某个 Agent 对一次工作分配的具体逻辑执行过程 |
| **启动记录 / 启动尝试** | `AgentLaunch` | `LaunchAttempt` | 系统为推进工作执行而实际启动一次具体 Agent 进程的记录 |
| **运行空间** | `RuntimeScope` | LSM `Logical Session` | LSM 为一次工作执行划定的安全隔离范围与权限作用域 |
| **执行任务** | `RuntimeJob` | LSM `Job` | 在运行空间内异步执行的具体机器任务，例如 `cargo test` |
| **持久终端 / 终端** | `PersistentShell` | `shell session` | 运行空间内维持环境和状态的常驻命令行交互终端 |
| **浏览器实例** | `BrowserInstance` | `browser session` | 运行空间内受控、有状态的浏览器实例 |
| **模型会话** | `ProviderThread` | `Provider Session` | Codex / Claude / Gemini 模型自身的连续私有对话流 |
| **会话** | `Session` | `Conversation` | 人类与特定 AgentInstance 之间的一对一持久工作会话；可为通用、项目或任务作用域，并跨多次 runtime/Run 持续存在 |
| **上下文快照** | `ContextSnapshot` | `ContextRevision` | 某次工作执行初始化时冻结的完整工作背景、目标与记忆视图 |
| **记忆** | `Memory` | `Memory` / `Context` | 跨任务与 Session 长期保存的知识，分 organization/project/Agent/task 等作用域 |
| **成果物** | `Artifact` | `Artifact` | 被正式归档、持久托管并可全局引用的工作交付物证据 |
| **决策记录** | `Decision` | `Decision` | 经确认、会约束或指导后续工作的技术或业务决策 |
| **工作交接** | `Handoff` | `Handoff` | 工作责任从一个执行转交至另一个执行的结构化交接协议 |
| **执行证据** | `WorkExecutionEvidence` | `RunExecutionEvidence` | 将工作语义状态与底层 LSM Audit / Job Logs 关联的追溯证据 |

完整规范见：

- [docs/agent-work-and-context-spec.md](docs/agent-work-and-context-spec.md)
- [docs/architecture.md](docs/architecture.md)

## 实际 Handoff 验证

M2.1 曾使用两个独立的 `codex-personal` CLI Session 和两个不同的 AgentInstance 做过完整验证。

Agent A 创建 typed artifact、decision、directed message 和 pending handoff；Handoff 创建后释放 A 的 Assignment，并将 A 的 Run 结束为 handed off。随后控制平面把工作分配给 Agent B 并创建它的 Run。Agent B **只通过员工 MCP** 重建状态、接受 Handoff、在原 thread 中回复、checkpoint 恢复后的状态并完成任务。

整个过程中，A 与 B 之间没有共享任何 Codex Session/chat history。

### 没有旧 token 时登录控制台

点击 WebUI 顶栏「登录控制台」→「通过 SSH 批准登录」。浏览器显示一个 10 分钟有效的
登录码。使用已有的服务器 SSH 访问权限，在 Morrows 服务目录执行：

```sh
morrows operator-approve --database data/morrows.db --code 页面显示的登录码
```

必须核对自己页面上的代码，不要批准陌生人发来的代码。命令默认授予 `operator`，有效期
24 小时；可显式指定 `--role viewer|operator|admin` 和 `--ttl-seconds`。这是真正的本地
管理操作，要求服务数据库的文件访问权限，没有公开的 HTTP 批准接口。重试同一个代码
不会扩权、续期或重复签发。批准后原浏览器自动登录，命令和 URL 都不包含 token。

登录请求本身不授予权限。数据库仅保存凭据摘要；批准只把浏览器持有秘密对应的摘要
登记为凭据。已撤销/到期凭据仍被拒绝，Agent 凭据不能充当 Operator。原有粘贴 token
登录保留，保存前会验证角色和有效性。默认仅在此标签页保存；勾选「记住登录」才存入
浏览器持久存储。界面显示实际认证结果、角色和到期时间，不再把任意非空字符串视为登录。
