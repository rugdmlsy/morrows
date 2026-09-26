# 用原生 Git 和文本工具管理记忆

CLI 只提供两个连接操作：从 Morrows 导入共享长期记忆，以及将一个已提交的文档发布回
Morrows。版本管理直接使用 Git；没有自制的 status、diff、提交图、工作区锁或恢复日志。

## 准备与多项目读取

从仓库构建 `cargo build -p morrows-cli`，可执行文件为 `target/debug/morrows`。
独立客户端使用已签发的 `MORROWS_AGENT_TOKEN` 和 `MORROWS_MCP_URL`；受管 runtime
可直接复用 `MORROWS_AGENT_AUTHORIZATION`（优先级高于 TOKEN），并通过 `MORROWS_MEMORY_CLI`
找到可执行文件的绝对路径，可用 `"${MORROWS_MEMORY_CLI:-morrows}"` 调用。凭据不写入记忆目录，
不扫描认证文件，不跟随 HTTP 重定向。需要本机 Git。

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

当前服务端还是 SQL：CLI 会遍历全部共享记忆历史页，沿 supersedes 链计算稳定目录名，
避免每次发布改文件名；`--history` 控制是否额外生成历史目录。它不是原子的项目快照；
发现重复 ID、缺失祖先或分叉会中止，不假装导入完整。Git 后端上线后可直接 fetch 项目 ref，
省掉 SQL 全量导入。这一过渡开销不产生全量 LLM prompt。

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
与旧知识是否已被替代。修改原始 metadata 会被拒绝；它不能替代认证或伪造来源。

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
