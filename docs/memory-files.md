# 用原生 Git 和文本工具管理记忆

CLI 只提供两个连接操作：从 Morrows 导入共享长期记忆，以及将一个已提交的文档发布回
Morrows。版本管理直接使用 Git；没有自制的 status、diff、提交图、工作区锁或恢复日志。

## 准备与多项目读取

从仓库构建 `cargo build -p morrows-cli`，可执行文件为 `target/debug/morrows`。
独立客户端使用已签发的 `MORROWS_AGENT_TOKEN` 和 `MORROWS_MCP_URL`；受管 runtime
可直接复用 `MORROWS_AGENT_AUTHORIZATION`（优先级高于 TOKEN），并通过 `MORROWS_MEMORY_CLI`
找到可执行文件的绝对路径，可用 `"${MORROWS_MEMORY_CLI:-morrows}"` 调用。凭据不写入记忆目录，
不扫描认证文件，不跟随 HTTP 重定向。需要本机 Git。发布前会核对服务端工具 schema 支持
稳定文档 UUID 与目标项目校验；旧服务端应先更新，不会静默忽略这些必要参数。

```sh
morrows memory checkout --project "$PROJECT_A" --root ./knowledge --history
morrows memory checkout --project "$PROJECT_B" --root ./knowledge
rg -n '超时|回滚' ./knowledge/projects/*/current -g 'content.*'
grep -R -n '约束' ./knowledge/projects
```

一个 `knowledge/.morrows.git` bare 仓库保存对象；`projects/<UUID>` 是原生 Git worktree。
每个项目有独立根提交与 `projects/<UUID>/main`、`projects/<UUID>/import` 分支。
import 分支记录服务端导入，main 保存本地工作，通过原生 Git 合并。项目 A 提交不改变 B。
本地使用带项目前缀的分支让 worktree 能直接执行普通 Git 命令；后续服务端 namespace
设计见 [memory-versioning.md](memory-versioning.md)，不假定本地所有 Git 命令都识别 namespace。

目录内 `current/<stable-key>/content.md` 保存逐字文本，结构化内容用 `content.json`；
`title.txt` 可修改，`metadata.json` 保留完整来源、验证状态与服务端 revision ID。
`--history` 将已替代版本的正文和 metadata 另存到 `history/`。查询输出只显示目录，
全文在文件中，Agent 可先检索相关段落再读取上下文。组织知识可读，员工不能改写它。

CLI 会遍历全部共享记忆历史页，沿 supersedes 链计算稳定目录名，
避免每次发布改文件名；`--history` 控制是否额外生成历史目录。SQL 后端尚未切换的项目不是原子的项目快照；
发现重复 ID、缺失祖先或分叉会中止，不假装导入完整。Git 后端通过 memory_head 检查分页快照一致性，仍走受权限保护的内容接口。这一过渡开销不产生全量 LLM prompt。

## 编辑、提交和发布

```sh
cd "knowledge/projects/$PROJECT_A"
sed -i.bak 's/旧描述/修正后的描述/' "current/$MEMORY_KEY/content.md"
git status
git diff -- "current/$MEMORY_KEY/content.md"
git add "current/$MEMORY_KEY/content.md"
git commit -m '根据复核结果修正描述，并保留适用限制'

morrows memory publish --dir . --memory "$MEMORY_KEY" --rev HEAD \
  --task "$SOURCE_TASK" --context-revision "$CONTEXT_READ" \
  --basis '本轮复核的结论与证据说明' --verification-status reported \
  --key "$PUBLICATION_UUID"
```

先读来源任务 context，把实际读到的 revision ID 用于发布。`verified` 必须有同任务的
`--artifact UUID` 或 `--decision UUID`；真实性仍由作者负责。正文字符串保留重复段落、
换行和不确定信息，JSON 必须有效。服务端绑定作者、项目和来源，检查写权限、上下文版本
与旧知识是否已被替代。CLI 还传入 `expected_project_id`，在发布事务内拒绝来源任务已被
移到其他项目的情况。修改原始 metadata 会被拒绝；它不能替代认证或伪造来源。

发布读取指定 commit 的原生 Git blob，**不读取尚未提交的工作文件**。成功返回完整回执、
来源 commit 与服务端 MemoryEntry ID。网络结果不确定时用同一 key、明确的 commit ID
和相同参数重试，复用服务端幂等记录；不要改为后来已经移动的 HEAD。没有第二套本地恢复协议。

本地 commit 不等于服务端发布。当前桥接每次发布一个文档，尚不是原子的多文档 Git push。
删除本地文件不删除服务端知识。任务 ContextRevision 和 Agent 私有记忆继续使用对应 MCP
工具；此项目 checkout 不导出其他 Agent 私有记录。

新增知识可直接创建 `current/<新 UUID>/`，放入 `title.txt`、`content.md`（或 content.json），
以及仅含 `{"draft":true}` 的 `metadata.json`，然后原生 `git add/commit` 并按上述命令发布。
CLI 以 `new_memory_id` 提交这个新 UUID，服务端在权限校验后拒绝 ID 冲突；刷新后文件路径
保持不变。新草稿发布后，把回执的完整 metadata（receipt 去掉 content，递归排序 JSON 键，
例如 `jq -S '.receipt | del(.content)'`）存回 metadata.json 并用 Git 提交，再 refresh；这可
避免本地 draft 标记与服务端来源记录产生 add/add 冲突。若已发生冲突，用原生 Git 保留
服务端完整来源，核对正文后解决。后续修订继续使用不可变版本及 supersedes 链。

## 刷新、历史与冲突

发布后、下一次修订前，导入最新服务端版本：

```sh
morrows memory checkout --project "$PROJECT_A" --root ./knowledge --refresh
git -C "./knowledge/projects/$PROJECT_A" log --oneline --all
git -C "./knowledge/projects/$PROJECT_A" diff HEAD~1 HEAD
```

刷新要求工作区干净；先提交或 stash 尚需保留的改动（包括编辑器产生的 `.bak`）。CLI
生成 import commit，再直接 `git merge`；冲突保留在 Git index/工作文件中，用普通 Git
解决后提交，或 `git merge --abort`。不会自行实现合并、静默重置或删除有用段落。
相同导入不创建重复提交；已选择的历史目录在后续刷新中继续保留。

`git branch`、`git log`、`git diff`、`git revert` 均可直接使用。用户本地的 Git 提交时间
表示何时导入/编辑，不等于远端事实的原始发生时间；原始时间与来源保存在 metadata 中。
当前所有分支共用一个可读对象库，不能把分支/namespace 当成不同用户的安全隔离。

共享提示词在 MCP initialize、Task launch、Session runtime 三处复用，明确主动查询历史、
主动沉淀知识及此文件工作流；不会把全部历史正文注入每次启动。

## 服务端 Git 权威存储（逐项目显式切换）

服务端现在支持 Git 后端；新建或尚未切换的项目仍以 SQL 为权威。操作员使用以下
control-plane API（需 Operator 凭据，不属于员工 MCP）逐项目迁移：

1. `POST /api/projects/<UUID>/memory/mirror`：建立只读镜像并返回 `verified_commit`。
   它不改变 SQL 正文或项目的读写后端。保留所有共享项目版本、原始 JSON 文本、完整正文、
   标题、UUID、scope/项目/任务/Agent 标识、来源、visibility、supersedes、原始时间字符串、
   provenance，以及 publication 的原始请求与证据引用。组织和私有知识不进入项目对象库。
2. `POST /api/projects/<UUID>/memory/cutover`，body 为 `{"verified_commit":"<commit>"}`。
   只有当前 SQL 全部记录与镜像逐字段完全一致，且实际 ref 仍是该已验证 commit，才切换。
   镜像后有任何 SQL 变化都必须重新 mirror。SQL 原记录保留，没有清空、摘要化或自动切换。
3. `POST /api/memory/reconcile` 可重试断点恢复；服务启动和记忆读写也执行恢复。

对象库在数据库旁，例如 `data/morrows.knowledge.git`。每个项目有独立的
`refs/heads/projects/<UUID>/main` 和独立根提交；导入时不会把旧 SQL 时间虚构成 Git 提交顺序。
Git `snapshot.json` 是完整原始记录，另有 `<revision UUID>.md/json` 原生正文 blob。
原始 supersedes 和时间保留在 manifest；镜像提交时间只是本次迁移时间。
备份必须同时保留数据库和 Git 目录。这个共享对象库只含共同可读的项目知识，ref 不是 ACL，
服务端不向员工开放任意 Git 对象访问。

切换后 `project_get.project.memory_head` 给出实际 Git head。CLI 把 checkout 所读 head
保存在 `project.json`，发布从指定的本地 commit 读取这个值作为 `base_commit`；不会偷偷读取
最新 head 来掩盖陈旧编辑。旧 checkout 必须 refresh，分页期间 head 改变会中止 checkout。
现有文件布局和原生 Git merge/revert/diff/log、rg、sed 工作流保持可用；CLI 仍通过受权限保护的
分页 API 导入内容，不开放裸对象库 fetch。每次发布仍是一个文档。

发布先在 SQL 提交 durable operation，再写 Git 对象及保留 ref，使用 `git update-ref` CAS
推进该项目 ref，最后事务性更新 SQL projection 和操作状态。SQL 与 Git **不是原子事务**。
若 CAS 后进程退出，重启依据实际 ref 和保留的 operation ref 幂等完成 indexing；意外 ref
移动会记录 conflict，绝不覆盖。相同 actor/key/payload 返回原回执，改变 payload 会冲突。
保留 operation refs 使未完成操作和旧版本经原生 Git GC 后仍可读。

Git 后端禁用共享项目的通用 SQL-only create 路径。读 projection 时持有应用 writer lock，
核对其 indexed_commit 和完整记录均与实际 Git head 一致；损坏或未完成索引时拒绝读取，
不会把竞争的 SQL 内容当成事实。恢复与切换是显式操作，不删除历史。

项目任务的 executor 在完成 Run 前还必须显式提交长期记忆处置。run_completion_check
返回 memory_disposition_template：有可复用知识时先调用 project_memory_publish，再以
published 或 updated 引用本任务实际产生的 MemoryEntry；updated 引用的记录必须
真实 supersede 旧项目记忆。确实没有可复用内容时使用 not_applicable 并说明原因。
更新 Task Context 本身不能代替项目长期记忆。Git 后端完成校验会确认当前 ref / projection
一致；如果该 publication 有 Git operation，则必须已经 indexed。切换前发布、随后经
完整 mirror 导入 Git 的旧 publication 没有 operation 行，仍可凭已验证的 authoritative
snapshot 通过，避免把迁移历史误判成未发布。

`POST /api/projects/<UUID>/memory/rebuild` 可以从已知权威 head 重建缺失或损坏的索引记录。
意外 head 移动或额外 SQL 记录必须先调查；rebuild 不认可未知 commit，也不删除额外证据。
