# Morrows Agent Delivery 协议

## 目标

Agent Delivery 是 Morrows 控制平面中的可靠投递 outbox。它解决的是“一个已经持久化的公司事件，是否已经被目标 Agent runtime 收到”，而不是替代原始业务对象。

原始事实仍由各自对象持有：

- 人类与 Agent 的正文：`SessionMessage`
- 管理指令正文：`LaunchInstruction`
- 工作状态：Task / Assignment / Run
- 长期恢复上下文：Context revision / ContextPackage / SessionSummaryRevision

`AgentDelivery` 只保存 source reference、目标 Agent、可选 Task 范围、transport payload 和投递状态。

## 状态语义

```text
queued -> claimed -> delivered
   \        |
    \-------+
     recover/release
```

- `queued`：事实已持久化，但还没有 transport 接收它。
- `claimed`：本地 launcher 已为某个具体 launch attempt 独占这批 delivery，正在把它们写入 provider turn。
- `delivered`：provider/bridge 已收到 payload。

`delivered` **不表示 Agent 已完成处理**。

例如 Session 的完整链路是：

```text
SessionMessage(status=queued)
        |
        +-- AgentDelivery(status=queued)
                 |
                 +-- transport received -> AgentDelivery(delivered)
        |
        +-- Agent replies -> SessionMessage(delivered)
```

因此 transport ack、消息已回复、Task 完成是三个不同状态。

## 事务性 outbox

以下写操作与 delivery 在同一个 SQLite transaction 中提交：

1. `create_human_session_message`
   - source: `session_message`
   - target: Session 的 `agent_instance_id`
   - task scope: none

2. `send_launch_instruction`
   - source: `launch_instruction`
   - target: launch attempt 的 `agent_instance_id`
   - task scope: launch attempt 的 Task

这意味着不会出现“消息已经保存，但 daemon 在创建通知前崩溃导致永远没人知道”的窗口。

## 旧数据与旧员工接口兼容

`0012_agent_delivery.sql` 会在升级时回填两类尚未完成的旧记录：

- `SessionMessage(status=queued)` → `session_message` delivery；
- active launch attempt 上已有的 `LaunchInstruction` → `launch_instruction` delivery。

已结束 launch attempt 的历史 instruction 不会被重新唤醒。

旧 employee MCP 路径继续可用，并与 outbox 保持一致：

- `session_get` 成功读取对话后，会消费该对话仍 queued 的 delivery；
- `session_reply` 在同一事务中消费相关 delivery 并把 human message 标记为已回复；
- `instructions_get` 成功读取 Task 指令后，会消费该 Task 对应的 queued instruction delivery。

因此旧 Agent 不需要先升级为显式 `delivery_inbox` loop，也不会因为新 outbox 被重复唤醒。显式 `delivery_ack` 对已经 delivered 的记录仍是幂等的。

WebUI 同时展示两个维度：消息是否已经到达 Agent runtime，以及是否仍在等待 Agent 回复。

## 本地 one-turn Provider：Codex / CodeBuddy

`codex exec` 与 CodeBuddy headless 都按“一次 provider turn 一个进程”管理。Morrows 不尝试向正在运行的 PID 伪造 live stdin；新的 delivery 在当前 turn 结束后通过 provider 原生 resume 进入下一 turn。

共同处理方式：

1. 当前 provider turn 运行期间，新 delivery 保持 `queued`。
2. 当前 turn 启动时，launcher 原子 claim：
   - 目标 Agent 相同；
   - delivery 没有 Task scope，或 Task 与当前 Run 相同。
3. delivery 被追加到 stdin prompt。
4. prompt 成功写入 provider 后，delivery 标记为 `delivered`。
5. 写入失败则 claim 回到 `queued`。
6. daemon 重启时，所有不再属于 starting/running attempt 的遗留 claim 自动回到 `queued`。
7. 如果当前 turn 退出、LSM-bound Run 进入 `interrupted` restart window，并且仍有 queued delivery，delivery worker 自动创建同一个 Run 的 restart attempt。
8. restart attempt 复用最近一个可用的 provider `external_session_ref`，因此下一条 delivery 是同一 provider thread 的新 turn，而不是新会话。

CodeBuddy 的额外边界：

- adapter 名称为 `codebuddy_cli`；`codebuddy_external` 仍保留给没有 active CLI bridge 的旧配置。
- prompt 通过 stdin 发送，任务正文不会进入 argv。
- 每次 launch 生成独立的 0600 MCP 配置文件，注入该 AgentInstance 的 Run-bound Morrows Bearer credential 和该 Run 的 LSM capability；argv 只出现配置文件路径。launch 结束时 runtime credential 被撤销。
- CodeBuddy 只开放 `ToolSearch` / `DeferExecuteTool` 两个内建调度工具。Bash/Write/Edit 等直接本地工具不可调用，系统副作用仍应通过 LSM MCP 产生并进入执行证据。
- headless 模式使用 CodeBuddy 的 non-interactive permission bypass 允许 deferred MCP 调用；该 bypass 不扩大上述内建工具白名单。
- 初始 turn 使用显式 session id，后续 turn 使用 CodeBuddy 原生 `--resume`。

Morrows 不会因为 delivery 创建第二个 Run，也不会创建第二个 LSM Logical Session。

## 外部 Provider Bridge

外部 provider 可使用 REST 或 Morrows employee MCP。

REST：

```text
GET  /api/agent-deliveries?limit=80
POST /api/agent-deliveries/{delivery_id}/ack
Authorization: Bearer mrw_agent_<secret>
X-Agent-Instance-Id: <agent uuid>   # optional binding; if present must match
```

MCP：

- `delivery_inbox(limit?)`
- `delivery_ack(delivery_id)`

Bridge 必须使用目标 AgentInstance 的 credential。其他 Agent 无法读取该 inbox，也无法 ack delivery。loopback 默认仍兼容旧的 `X-Agent-Instance-Id`-only bridge；设置 `MORROWS_REQUIRE_AGENT_AUTH=1` 后该兼容路径关闭。bridge credential 由本地控制面显式签发/撤销，数据库只保存 token 的 SHA-256 hash。

典型 bridge loop：

```text
poll delivery_inbox
    |
    +-- session_message
    |      -> load session_get
    |      -> feed provider
    |
    +-- launch_instruction
           -> load instructions_get / payload
           -> feed provider
    |
provider accepted input
    -> delivery_ack
```

如果 provider 本身支持 event/webhook，可由 provider-specific bridge 用事件触发替代轮询，但 Morrows 的 durable delivery 语义不变。

## 并发与重复

本地 launcher 使用 `claimed_by_launch_attempt_id` 防止同一个 queued delivery 被多个本地 turn 同时消费。

外部 bridge 当前采用显式 inbox + ack，语义是 at-least-once。bridge 在 ack 前崩溃时可能再次看到同一 delivery，因此 provider-specific bridge 应使用 `delivery.id` 做幂等键。

## 不负责的事情

Agent Delivery 不负责：

- 判断 Agent 是否理解或完成了消息；
- 取代 SessionSummary / ContextPackage；
- 取代 Run checkpoint / complete；
- 推断 provider quota；
- 启动没有 active bridge 的第三方桌面应用；
- 绕过 Morrows 的 Assignment / Run / LSM 生命周期。
