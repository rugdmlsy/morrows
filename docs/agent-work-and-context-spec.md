# Morrows Agent Work & Context Specification v1

## 1. 目标与定位

本规范定义 Morrows 中 Agent 工作过程的统一数据模型、持久化边界、术语规范与协作协议。使不同 Provider 的 Agent（Codex、Claude、Gemini、CodeBuddy、OpenCode 等）可以：

- **统一工作协议**：使用一致的 Agent-native 协议与心智模型开展工作；
- **跨 Provider 无损交接**：在不同模型与厂商之间迁移和交接工作，无需共享 Provider 私有对话记录；
- **可审计与可复现**：保存可追溯、可审计的工作过程与结构化产物证据；
- **知识与私有状态分离**：共享企业/项目/任务的持久知识，隔离底层执行资源与模型私有会话；
- **独立于临时环境的工作恢复**：即使原 Agent 进程崩溃、机器宕机或临时工作空间被清理，新接手的 Agent 仍可凭借 Morrows 规范数据完全恢复并继续工作。

> **终极核心原则 (Ultimate Invariant)**：  
> **Morrows 是工作语义与持久知识的 Source of Truth；LSM 是执行证据与运行资源的 Source of Truth；Provider Session 是模型会话的 Source of Truth。**  
> 任何一层不得替代另一层。即使原 Agent、Provider Session、执行机器和临时目录完全消失，新 Agent 依然能仅依靠 Morrows 的规范对象完整恢复工作状态。

---

## 2. 三平面系统边界 (Three-Plane Architecture)

系统在逻辑与物理上清晰划分为三个平面：

```
┌─────────────────────────────────────────────────────────────────────────┐
│ 1. Morrows Control Plane (控制平面)                                      │
│    工作语义与持久知识的 Source of Truth                                    │
│    负责：工作项、工作分配、工作执行生命周期、上下文快照、记忆、成果物托管、           │
│          决策记录、工作交接、工作对话、摘要聚合、执行证据关联等                │
└────────────────────────────────────┬────────────────────────────────────┘
                                     │
                     /api/control    │ (受控 loopback)
                     scoped token    ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ 2. LSM Runtime Plane (运行平面)                                         │
│    执行证据与宿主运行资源的 Source of Truth                                 │
│    负责：运行空间 (RuntimeScope)、执行任务 (RuntimeJob)、持久终端 (Shell)、    │
│          浏览器实例、文件系统/远程机器操作、底层审计日志 (Audit) 等             │
└────────────────────────────────────┬────────────────────────────────────┘
                                     │
                 scoped capability   │ (约束执行环境)
                                     ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ 3. Provider Plane (模型平面)                                            │
│    模型推理与私有交互会话的 Source of Truth                                  │
│    负责：Codex / Claude / Gemini / CodeBuddy 的模型上下文、Rollout /     │
│          私有对话流、内部思考过程 (Reasoning)、断点恢复 (Resume State) 等    │
│    * 声明：Provider 会话属于外部私有数据，不作为 Morrows 标准状态共享           │
└─────────────────────────────────────────────────────────────────────────┘
```

* **Morrows Control Plane 回答**：要做什么（What）、谁负责（Who）、做到哪里（Progress）、定了什么（Decisions）、正式交付了什么（Artifacts）、哪些知识保留给未来（Memory）。
* **LSM Runtime Plane 回答**：Agent 实际执行了什么操作、在哪台机器、调用了什么工具、命令真实输出是什么、资源使用情况。LSM 不判断工作项语义上的成败。
* **Provider Plane 回答**：大模型当下的推理链条、私有会话历史、模型供应商特定的交互参数。

---

## 3. 去歧义术语规范与对象模型 (Naming & Object Normalization)

为彻底解决传统系统中 `session`、`run` 等高频词滥用导致的严重歧义（如“恢复会话”到底是恢复 Codex 会话、LSM 会话还是 Morrows 记录？），规范确立**两层命名法**：
1. **用户层 / 交互层命名**：面向人类前端（WebUI）、日常协作及人机 Prompt 交互，借鉴飞书等成熟协同产品的心智模型，使用贴近组织协作的词汇；
2. **系统层命名**：面向底层数据库、API 契约与内部数据结构，严谨且一对一映射。

### 3.1 术语对照映射表

| 用户层 / 沟通词汇（推荐） | 系统层正式英文 | 原旧词 / 易混淆词 | 核心定义与语义范围 |
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
| **成果物** | `Artifact` | `Artifact` | 被正式归档、持久托管并可全局引用的工作交付物证据 |
| **决策记录** | `Decision` | `Decision` | 经确认的、对后续工作产生约束与指导的技术或业务决断 |
| **工作交接** | `Handoff` | `Handoff` | 工作责任从一个执行转交至另一个执行的结构化交接协议 |
| **执行证据** | `WorkExecutionEvidence` | `RunExecutionEvidence`| 将工作语义状态与底层 LSM Audit、Job Logs 关联的追溯证据 |

### 3.2 Agent 对话与指令规范

在 Agent 交互、系统 Prompt、MCP 工具说明及日志中，**严禁裸用 `session` 和 `run`**，统一采用标准术语：
* ❌ 严禁使用：“请恢复上次 session”、“这个 run 执行完了没有”、“更新当前 context”。
* ✅ 规范表述：
  * “继续推进工作项 [重构 WebUI]”
  * “查看工作执行 [exec-01] 的状态”
  * “在运行空间 [scope-01] 中启动持久终端”
  * “恢复对应的模型会话 [thread-abc]”
  * “生成新的上下文快照并完成工作交接”

---

## 4. 知识与记忆层级体系 (Memory, Context & Summary)

知识体系分为四种截然不同的形态，严禁相互替代与污染：

```
                ┌───────────────────────────────────┐
                │ 长期记忆 (Long-Term Memory)        │
                │ 组织级 / 项目级 / 员工级 / 工作项级     │
                └─────────────────┬─────────────────┘
                                  │
                                  ▼ Materialize (装配)
                ┌───────────────────────────────────┐
                │ 上下文快照 (ContextSnapshot)        │
                │ 不可变冻结快照，绑定至 WorkExecution  │
                └─────────────────┬─────────────────┘
                                  │
                                  ▼ Pinned by
                        WorkExecution (工作执行)
                                  │
         ┌────────────────────────┴────────────────────────┐
         ▼                                                 ▼
┌─────────────────────────────────┐               ┌─────────────────────────────────┐
│ 结构化摘要 (Structured Summary) │               │ 成果物 (Managed Artifact Store) │
│ 确定性事实 + 经证据背书的语义提炼 │               │ 脱离本地临时工作目录，持久留存受管副本 │
└────────────────┬────────────────┘               └─────────────────────────────────┘
                 │
                 ▼ Assemble
┌───────────────────────────────────────────────────┐
│ 上下文包 (Context Package)                        │
│ 跨 Agent 共享与无缝工作交接的标准承载体               │
└───────────────────────────────────────────────────┘
```

### 4.1 长期记忆 (Memory)
* **作用域分层**：
  * **组织记忆 (Organization Memory)**：企业规范、架构通则、通用工作流。
  * **项目记忆 (Project Memory)**：特定项目的业务背景、代码库设计原则与技术栈要求。
  * **员工记忆 (Agent Memory)**：特定 Agent 实例的工作习惯、特长与工具偏好。
  * **工作记忆 (Task Memory)**：围绕某一特定工作项累积的长期经验与核心背景。
* **准入准出原则**：长期记忆通过版本追加（Supersede）演进。**单次 Tool Output、冗长 Shell 输出、临时调试路径及模型推测严禁直接灌入长期记忆**。

### 4.2 上下文快照 (ContextSnapshot)
* 在一次 `WorkExecution` 启动时，由系统根据工作项定义、相关 Memory、关联 Decision、前序交接成果物物化而成；
* **完全不可变 (Immutable)**：一旦绑定至某次工作执行，永远保持不变，用于精准复盘：“该 Agent 在执行当时究竟看到了什么信息”。

### 4.3 结构化摘要 (Structured Summary)
摘要属于派生数据，不是 Source of Truth。严禁让 Agent 自由生成随意散漫的 Markdown 总结，必须分层生成：
1. **确定性系统事实 (Deterministic Facts)**：由系统直接采集（修改的文件列表、Git Commits、执行的测试命令、退出码与测试结果、产生的成果物等），模型无权篡改；
2. **语义提炼 (Semantic Extraction)**：由模型提取工作目标、关键发现、未决问题与阻碍，但**关键结论必须挂载 Evidence 引用**（如关联的文件行、Audit 事件 ID）。无 Evidence 的推论显式标记为 `unverified`。

### 4.4 上下文包 (Context Package)
跨 Agent 或跨 Provider 移交工作时的标准化交付物：
```text
ContextPackage:
├── WorkItem & Objective
├── Structured Summary (确定性事实 + 语义结论)
├── Pinned ContextSnapshot ID
├── Confirmed Decision References
├── Managed Artifact References (归档 URI / 元数据引用)
├── Changed Files & Verification Status
├── Blockers & Unresolved Questions
└── Suggested Next Action
```

---

## 5. 成果物托管与执行证据 (Artifacts & Execution Evidence)

### 5.1 托管成果物 (Managed Artifact Store)
* 本地工作区的临时代码或文件并不自动等同于 `Artifact`；
* 当 Agent 调用产物发布接口时，Morrows 复制并归档目标文件至受管存储区（`data/artifacts`），记录存储 URI、MIME 类型、文件大小和版本（无需计算 SHA-256 哈希值）；
* 即使执行完毕后工作机器的 `/tmp` 或工作区被删除，后续接替的 Agent 依然可以稳定访问该成果物。

### 5.2 执行证据 (WorkExecutionEvidence)
* 将上层语义执行（`WorkExecution`）与底层 LSM 产生的具体审计事件、Job 日志流、Shell 执行记录进行绑定关联；
* 确保上层声称的“测试通过”或“修复完成”具有底层不可篡改的执行证据支撑。

---

## 6. 标准 Agent 工作协议 (`morrows-work` Skill)

所有接入 Morrows 的 Agent 实例均遵循标准化工作流程：
1. **读取任务 (Read WorkItem)**：通过 `task_get` 获取工作项目标、边界与验收标准；
2. **加载上下文快照 (Read ContextSnapshot)**：获取冻结的工作背景；
3. **检索长期记忆与决策 (Read Memory & Decisions)**：获取项目规范与已确认决策；
4. **获取管理指令 (Read Instructions)**：查看人类下达的阶段性补充要求；
5. **在运行空间内执行 (Execute in RuntimeScope)**：通过 LSM 工具执行命令、操作代码与运行测试；
6. **定期保存检查点 (Checkpoint)**：持久化阶段性成果与当前断点；
7. **可验证验收 (Verification)**：必须以客观事实（测试通过、构建成功、UI 渲染无误）验证，严禁仅凭修改了文件即判定完成；
8. **发布成果物与决策 (Publish Artifacts / Decisions)**：将关键文件与重要架构决断归档进系统；
9. **提交自述报告 (Self Report)**：结构化反馈完成内容与阻碍；
10. **交接或完成 (Complete or Handoff)**：未完结则发起 `Handoff` 释放租约，完结则调用 `run_complete`。
