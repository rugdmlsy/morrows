# 长期记忆的多项目 Git 管理设计

状态：已实现逐项目 Git 镜像、完整等价校验、显式 cutover、CAS 发布与断点索引恢复。操作说明见 memory-files.md。下文保留目标设计；当前采用 refs/heads/projects/<UUID>/main 独立根，而非 GIT_NAMESPACE；远端分支合并和多项目 manifest API 尚未实现，本地原生 Git 工作流可用。

## 目标与已有基础

Agent 在同一个工作目录管理多个项目的知识，每个项目独立提交、分支、合并和回退。
项目 A 的提交不改变项目 B 的版本。无需为每个项目手工创建、配置和维护仓库，也不把
所有项目捆在一个全局 main 上。Git 已有的对象、提交图、差异、合并和回退语义直接复用。

当前 MemoryEntry 已保留不可变内容、来源、验证声明和 supersedes 链；项目参与者通过
所属任务直接发布。Git 后端每次发布创建项目整体 commit；远端分支或 merge API 尚未开放。当前 CLI 的文件接口见
[memory-files.md](memory-files.md)，不把本地修改伪装成已经写入服务端的事实。

## 存储：共享对象库，各项目独立引用与历史

在一个共同可读的安全域中，由 Morrows 管理一个 bare Git repository。每个项目使用
UUID 派生的**扁平 namespace**，拥有独立的根提交、分支和标签。项目名可修改，UUID 不变。

```text
knowledge.git/
  objects/                         # Git 管理的共享对象库
  refs/namespaces/p-<project-A>/refs/heads/main
  refs/namespaces/p-<project-A>/refs/heads/experiment
  refs/namespaces/p-<project-A>/refs/tags/reviewed-2026-09
  refs/namespaces/p-<project-B>/refs/heads/main
  refs/namespaces/organization/refs/heads/main
  refs/morrows/snapshots/<snapshot-id>
```

项目 A/B 是不相交的提交图，不是同一提交中的两个目录，也不是普通 main 派生的两个分支。
它们可以有相同的分支名。仓库备份、维护和对象回收统一处理，项目选择与版本选择独立。
删除项目只停止展示或归档引用，不顺带清除其他项目对象；保留策略另行显式执行。

Git 官方的 namespace 本就支持把一个仓库中的引用隔离为多个逻辑仓库，并共享对象库。
使用扁平 `p-UUID` 避免斜线 namespace 的递归展开。服务器内部 plumbing 显式使用完整
ref；不能假设所有 Git 命令都自动遵循 `GIT_NAMESPACE`。
[Git namespaces 官方文档](https://git-scm.com/docs/gitnamespaces)

**Namespace 不是访问控制。** 共同可读的项目可以共用对象库；Agent 私有知识或互不可读
的组织必须放在不同安全域的对象库中。员工接口只返回授权的文件、差异和提交元数据，
不向不可信客户端开放底层裸 Git fetch/push，也不接受任意对象 ID 读取整个库。
项目查询仍默认展示分配给该 Agent 的项目/任务，但不以分配关系拒绝读取其他共享项目。

## 提交中的文件与来源

```text
project.json                      # project_id、格式版本
memories/<stable-memory-id>/
  content.md 或 content.json       # 原生 UTF-8 正文；保留原始数据类型
  metadata.json                    # 标题、来源、验证状态、适用边界、证据引用
```

稳定 memory ID 标识一条知识，commit 标识项目知识集版本；替代关系不再迫使文件改名。
多行 JSON 仅规范序列化，正文不做摘要化、删重或截断。每次提交可修改同项目多条知识。
作者、来源 Task、ContextRevision、修改理由、原始来源时间、Artifact/Decision 引用和
幂等 operation ID 作为元数据保留。认证身份由服务端绑定；不能仅信任客户端填写的 Git author。
旧版本始终可从历史读取。回退创建反向提交，不抹掉原提交或已发生的审计记录。

## 原生操作与 Morrows 薄适配层

| 操作 | 复用 Git | Morrows 补充的规则 |
| --- | --- | --- |
| 阅读/搜索 | tree、show、工作目录、grep/rg | project/branch/commit 定位、授权、当前与历史分开 |
| 提交 | write-tree、commit-tree、update-ref | 校验来源与内容 schema；认证作者；要求读过的 base_commit |
| 历史/差异 | log、diff、blame | 默认仅指定项目；筛选记忆路径、任务和作者；分页返回 |
| 分支/标签 | 原生 refs | project_id 映射完整 ref；验证名称，不允许路径逃逸或跨项目父提交 |
| 合并 | merge-tree --write-tree | 限同项目共同历史；保留冲突；通过领域校验后才提交并更新目标分支 |
| 回退 | revert | 创建新提交；保留旧来源，并注明撤销的结论及原因 |
| 选择多个项目 | 固定 commit 的 manifest | 每个项目独立版本；一次显式保存某组项目的版本组合 |

并发提交使用 `git update-ref <ref> <new> <expected-old>`。旧 head 不匹配时保留工作副本，
返回新的 head 和可阅读的差异。可在已知共同祖先上尝试 Git 三方合并；发生冲突时不自动
覆盖、不强推、不降低验证状态要求。使用隔离的临时 index/worktree，避免不同项目的并发
请求共用同一个 index。对象 ID 作为 Git 的不透明标识使用，无需另做内容校验流程。
[Git update-ref 官方文档](https://git-scm.com/docs/git-update-ref)

`merge-tree --write-tree` 不修改工作树，可复用 Git 内容及重命名合并逻辑。单次调用退出码
0 为干净合并、1 为冲突，其他值为错误；保留完整冲突信息，不能只检查文本冲突标记。
Git 文本合并成功后仍须检查元数据、任务证据归属及记录结构，不能宣称它验证了事实真假。
不同项目之间不使用 `--allow-unrelated-histories` 自动混合历史。
[Git merge-tree 官方文档](https://git-scm.com/docs/git-merge-tree)

跨项目复用一条知识时，显式导入内容并记录来源 project/commit/memory ID 与适用限制；
不要直接把 A 的 head 作为 B 的父提交。共享组织知识使用自己的 namespace，各项目可
在执行快照中固定所读取的组织版本。跨项目搜索可批量查目录或索引，但结果保留项目归属。

## 多项目版本组合与一致性

单项目分支提交是常规操作，不产生全局版本。需要复现实验或交接时，显式创建组合快照：

```json
{
  "projects": {
    "project-A": "commit-A7",
    "project-B": "commit-B3"
  },
  "organization": "commit-O2"
}
```

快照不可变，固定各自的版本，读取时跟随这些对象而非逐个查询活动 main。用专门保留 refs
固定 manifest 引用的项目 commits，防止 Git GC 把只在 JSON 字符串里出现的对象回收；
JSON 中记录 OID 本身**不会**建立 Git 对象可达关系。

需要同时发布多个项目 head 时，可使用 `update-ref --stdin` 的 prepare/commit 事务。
但官方明确指出，并发读者仍可能观察到部分 refs 的变化。因此跨项目一致读取必须以一个
已发布的 immutable manifest 为入口；不声称多引用更新为读者提供了数据库式快照隔离。
初期不提供“全局回滚所有项目”的隐含动作；组合回退必须明确列出项目与旧/新 commit。

## Git 与数据库之间的职责和故障恢复

Git 保存内容与版本图，是版本事实源；SQLite 保存授权、项目到安全域/namespace 的映射、
检索索引和发布操作日志。不要再在 SQL 中实现一套 commit/branch/merge 算法。

一次发布先持久化 operation ID、请求内容、base head 和待写入内容，再生成 Git commit，
保存 resulting OID，最后 CAS 更新项目 ref。Git head 的成功更新是版本发布的线性化点。
幂等重试读取操作日志与 ref/提交中的 operation ID 来判定是否已成功。随后重建受影响的
索引并记录完成；API 返回真实提交状态及索引滞后状态，不能因索引失败宣称 Git 提交没发生。
索引结果带 commit OID；与所查询的 head 不同则读取对应 Git 内容或等待索引修复。

Git ref 和 SQLite 不是一个原子事务。恢复器逐项对账：写对象后未更新 head 的操作可按
原 base 重试；CAS 失败的对象不当作已发布版本；head 已推进而索引尚未更新则补建索引。
待恢复对象用 pending refs 保留，成功/明确放弃后按策略回收。禁止通过 reset 重写已发布
历史来掩盖部分成功。授权和来源校验在发布锁下复核；同项目序列化短暂的最终发布阶段。

## 员工体验与分阶段落地

当前已实现：`memory checkout`、`memory publish`，同一个 root 共享 bare 对象库，按 project
UUID 创建原生 worktree 与独立根历史。本地直接使用 Git 的 status/diff/log/add/commit/
branch/revert；刷新直接 git merge，Git 管理 index、对象、CAS 引用锁和冲突。原生
`rg/grep/sed` 编辑正文，发布只读指定 commit 的 blob，并复用现有 employee MCP 的
单条幂等、权限与旧版本冲突检查。未实现服务端 Git 后端；凭据不写入记忆目录。

本地 ref 为 `refs/heads/projects/<UUID>/{main,import}`，方便普通 worktree 命令直接运行；
服务端对外逻辑项目采用上面的 namespace 布局。两者都让项目有独立提交图，不要求 Agent
自己创建和维护多个 repo。工作区状态与锁由 Git 管理，发布恢复使用已有服务端幂等逻辑，
不另外实现 Git 的替代系统。

下一阶段采用 Git 后端后，保留同样的文本目录和原生 Git 命令，连接服务端 ref 的
fetch/publish 与 `snapshot create/show`；checkout 显式记录每个项目的 base_commit。
批量选项目只控制工作区可见范围，不把它们捆成一个提交。分支/冲突结果优先落入文件，
Agent 先检索相关段落，再把必要内容读进上下文，不把全量历史塞进每次启动提示词。

迁移先只读镜像、逐项目核对记录数量、完整内容和来源，再切换该项目的发布入口。
旧 MemoryEntry UUID、supersedes 和原始时间保留为迁移来源；旧记录缺少真实提交分组或
排序证据时标明未知，不编造“当时的项目快照”。切换期间每个项目仅一个发布事实源，
不得让 SQL 与 Git 双写各自成功、出现两套 current。回退迁移只切换入口并完整保留两侧证据。

验收覆盖：A/B 独立根与 head；改 A 不改变 B；跨项目 grep；CAS 拒绝旧 head；原生 merge
干净与冲突两种路径；revert 保留历史；原生文本往返不丢正文；跨项目错误写入被拒；
断线重试不重复发布；历史/来源可读；组合快照固定版本并经 GC 后仍可读。
