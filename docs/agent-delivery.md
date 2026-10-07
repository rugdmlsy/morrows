# Agent Delivery

## 目标

`AgentDelivery` 是 Morrows 的事务型投递 outbox。它不定义会话身份，也不保存另一套聊天状态。

活跃 delivery 只有两种：

- `task_message`：Human 写入 Task collaboration 后，通知目标 AgentInstance 有新消息。
- `launch_instruction`：控制平面向某个 LaunchAttempt 发送的管理指令。

Task 消息正文和作者信息保存在 `MessageThread` / `Message`。Delivery 只保存可靠投递状态和 source reference。

## Task message

一个 Task + AgentInstance 最多有一个 `MessageThread(kind=human_agent)`。Human 与 Agent 对话都写入该 thread。

Human 消息创建时：

1. 写入 `Message(author_type=human)`；
2. 创建 `AgentDelivery(kind=task_message)`；
3. payload 只包含 `thread_id`、`message_id` 和 bounded body；
4. delivery 在被 runtime claim 前保持 `queued`；
5. Human 可以在 claim 前撤回消息。

Agent 回复时使用同一 Task thread。回复会消费相关 queued delivery，并追加 `Message(author_type=agent)`。

知道 `thread_id` 或 Provider Session ID 都不会授予 Task 写权限。Task 写权限只来自 Task ownership 或 Assignment history。

## Launch instruction

`launch_instruction` 绑定 Task 和 LaunchAttempt。受管 launcher 只 claim 自己 Task 的 `launch_instruction` 与 `task_message`。其他 Task 的消息不会泄漏进当前 launch，也不会唤醒无关 Run。

## 状态语义

- `queued`：尚未被受管 runtime/bridge claim；
- `claimed`：已分配给正在处理的 LaunchAttempt；
- `delivered`：runtime/bridge 已接收并处理 source。

如果 LaunchAttempt 异常结束，Morrows 会把遗留的 `claimed` delivery 重新放回 `queued`。

## Provider continuity

Delivery 不保存 Provider conversation identity。唯一权威 Provider continuity 是 `Run.provider_conversation_ref`。

Codex / Claude / CodeBuddy 的 session/thread/conversation ID 属于 Provider Plane。它不等于 Task、Run、MessageThread 或 RuntimeScope。

## 不负责的事情

AgentDelivery 不负责创建或恢复 Provider conversation，不负责 Task write authorization，不保存长期 Project Memory，不表示 RuntimeScope，也不保存 Human/Agent transcript 的第二份副本。
