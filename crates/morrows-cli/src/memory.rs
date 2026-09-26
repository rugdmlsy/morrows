use crate::client::Client;
use anyhow::{Context, Result, bail, ensure};
use clap::Subcommand;
use morrows_core::{MemoryEntry, PublishProjectMemory};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command as Process, Stdio},
};
use uuid::Uuid;

#[derive(Subcommand)]
pub enum Command {
    /// Import shared knowledge into native Git worktrees in ROOT/projects/PROJECT.
    Checkout {
        #[arg(long)]
        project: Uuid,
        #[arg(long, default_value = "morrows-memory")]
        root: PathBuf,
        /// Materialize retained superseded bodies in history/ as well as current/.
        #[arg(long)]
        history: bool,
        /// Fetch a new import commit and use native git merge; requires a clean worktree.
        #[arg(long)]
        refresh: bool,
    },
    /// Publish one COMMITTED document through employee MCP. Use native git commands to edit/review/commit.
    Publish {
        #[arg(long)]
        dir: PathBuf,
        /// Stable document directory UUID under current/.
        #[arg(long)]
        memory: Uuid,
        /// Commit to publish; pin the reported commit when retrying an uncertain response.
        #[arg(long, default_value = "HEAD")]
        rev: String,
        #[arg(long)]
        task: Uuid,
        #[arg(long)]
        context_revision: Uuid,
        #[arg(long)]
        basis: String,
        #[arg(long, value_parser = ["reported", "verified", "hypothesis", "unverified"])]
        verification_status: String,
        #[arg(long)]
        artifact: Vec<Uuid>,
        #[arg(long)]
        decision: Vec<Uuid>,
        /// New UUID per publication; repeat the same key, commit and arguments after an uncertain response.
        #[arg(long)]
        key: Uuid,
    },
}

pub async fn run(command: Command) -> Result<()> {
    match command {
        Command::Checkout {
            project,
            root,
            history,
            refresh,
        } => {
            let client = Client::connect().await?;
            let (project_data, entries) = fetch_project(&client, project).await?;
            let dir = checkout(
                &root,
                project,
                &client.endpoint,
                project_data,
                entries,
                history,
                refresh,
            )?;
            println!(
                "{}",
                json!({"directory":dir,"status":"ready","version_control":"native git"})
            );
        }
        Command::Publish {
            dir,
            memory,
            rev,
            task,
            context_revision,
            basis,
            verification_status,
            artifact,
            decision,
            key,
        } => {
            let dir = dir.canonicalize()?;
            let commit = git(
                &dir,
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{rev}^{{commit}}"),
                ],
            )?;
            // Read immutable Git blobs, never partially edited working files.
            // The same commit/key/arguments reproduce the exact server request;
            // server idempotency handles a lost response, without a second local journal.
            let project_data = show_json(&dir, &commit, "project.json")?;
            let project: Uuid = serde_json::from_value(project_data["project"]["id"].clone())?;
            let metadata = show_json(&dir, &commit, &format!("current/{memory}/metadata.json"))?;
            let import_ref = format!("refs/heads/projects/{project}/import");
            let base = git(&dir, &["merge-base", &commit, &import_ref])?;
            let metadata_path = format!("current/{memory}/metadata.json");
            let imported = git(
                &dir,
                &["ls-tree", "--name-only", &base, "--", &metadata_path],
            )?;
            if imported.is_empty() {
                ensure!(
                    metadata == json!({"draft":true}),
                    "new documents must use metadata.json containing only draft: true"
                );
            } else {
                ensure!(
                    metadata == show_json(&dir, &base, &metadata_path)?,
                    "source metadata changed; retain imported metadata and edit content/title only"
                );
            }
            let supersedes = if metadata["draft"] == true {
                None
            } else {
                ensure!(
                    metadata["scope_type"] == "project"
                        && metadata["project_id"] == json!(project)
                        && metadata["visibility"] == "shared",
                    "only shared knowledge in this project can be published"
                );
                Some(serde_json::from_value::<Uuid>(metadata["id"].clone())?)
            };
            let paths = git(
                &dir,
                &[
                    "ls-tree",
                    "--name-only",
                    &commit,
                    "--",
                    &format!("current/{memory}/"),
                ],
            )?;
            let markdown = format!("current/{memory}/content.md");
            let structured = format!("current/{memory}/content.json");
            let has_markdown = paths.lines().any(|p| p == markdown);
            let has_json = paths.lines().any(|p| p == structured);
            ensure!(
                has_markdown != has_json,
                "commit exactly one content.md or content.json for this document"
            );
            let content = if has_markdown {
                Value::String(show(&dir, &commit, &markdown)?)
            } else {
                show_json(&dir, &commit, &structured)?
            };
            let request = PublishProjectMemory {
                task_id: task,
                idempotency_key: key.to_string(),
                new_memory_id: if supersedes.is_none() {
                    Some(memory)
                } else {
                    None
                },
                title: show(&dir, &commit, &format!("current/{memory}/title.txt"))?,
                content,
                verification_status,
                basis,
                context_revision_id: context_revision,
                artifact_ids: artifact.into_iter().map(|id| id.to_string()).collect(),
                decision_ids: decision.into_iter().map(|id| id.to_string()).collect(),
                supersedes_memory_id: supersedes,
            };
            let client = Client::connect().await?;
            ensure!(
                project_data["endpoint"] == client.endpoint,
                "checkout belongs to a different MORROWS_MCP_URL"
            );
            let source = client.call("task_get", json!({"task_id":task})).await?;
            ensure!(
                source["project_id"] == json!(project),
                "source task belongs to another project"
            );
            eprintln!(
                "Publishing commit {commit}, document {memory}, key {key}; retry with this exact commit and arguments if the result is uncertain."
            );
            let receipt = client
                .call("project_memory_publish", serde_json::to_value(request)?)
                .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "status":"published","source_commit":commit,"local_key":memory,"receipt":receipt,
                    "next":"Use memory checkout --refresh to merge updated server metadata before publishing the next revision."
                }))?
            );
        }
    }
    Ok(())
}

/// The SQL compatibility API exposes revision IDs, not stable document IDs.
/// Fetch retained lineage to derive stable filenames across imports; bodies go
/// to disk/Git, not to the LLM context. Git-backed storage can replace this read
/// with a fetch of the project's ref without changing the native worktree UX.
async fn fetch_project(client: &Client, project: Uuid) -> Result<(Value, Vec<MemoryEntry>)> {
    let mut offset = 0;
    let mut entries = Vec::new();
    let mut seen = HashSet::new();
    loop {
        let response = client
            .call(
                "project_get",
                json!({"project_id":project,"include_superseded":true,"limit":100,"offset":offset}),
            )
            .await?;
        ensure!(
            response["project"]["id"] == json!(project),
            "unexpected project response"
        );
        let page: Vec<MemoryEntry> = serde_json::from_value(response["memory"]["items"].clone())?;
        for entry in page {
            ensure!(
                entry.visibility == "shared"
                    && (entry.scope_type == "organization"
                        || (entry.scope_type == "project" && entry.project_id == Some(project))),
                "unexpected memory scope"
            );
            ensure!(
                seen.insert(entry.id),
                "memory changed during pagination; retry checkout"
            );
            entries.push(entry);
        }
        match response["memory"]["next_offset"].as_u64() {
            Some(next) if next > offset => offset = next,
            Some(_) => bail!("invalid continuation offset"),
            None => return Ok((response["project"].clone(), entries)),
        }
    }
}

fn checkout(
    root: &Path,
    project: Uuid,
    endpoint: &str,
    project_data: Value,
    entries: Vec<MemoryEntry>,
    mut history: bool,
    refresh: bool,
) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    let root = root.canonicalize()?;
    let bare = root.join(".morrows.git");
    let dir = root.join("projects").join(project.to_string());
    let import_ref = format!("refs/heads/projects/{project}/import");
    let branch = format!("projects/{project}/main");
    if !bare.exists() {
        git(
            &root,
            &["init", "--bare", bare.to_str().context("UTF-8 path")?],
        )?;
    }
    ensure!(
        git(&bare, &["rev-parse", "--is-bare-repository"])? == "true",
        "memory root must contain a bare Git repository"
    );
    if dir.exists() {
        ensure!(
            refresh,
            "checkout exists; use --refresh to merge imports with native Git"
        );
        let common = git(
            &dir,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        ensure!(
            Path::new(&common).canonicalize()? == bare.canonicalize()?,
            "directory belongs to another Git repository"
        );
        ensure!(
            git(&dir, &["status", "--porcelain"])?.is_empty(),
            "commit or stash local changes before refreshing; no files were overwritten"
        );
        let saved = show_json(&dir, "HEAD", "project.json")?;
        ensure!(
            saved["endpoint"] == endpoint && saved["project"]["id"] == json!(project),
            "checkout belongs to another endpoint/project"
        );
        history |= saved["includes_history"] == true;
    } else {
        ensure!(
            !refresh,
            "no checkout exists; omit --refresh for the first import"
        );
    }
    let old = optional_ref(&bare, &import_ref)?;
    let temp = Temp::under(&root)?;
    write_json(
        &temp.0.join("project.json"),
        &json!({"format_version":1,"endpoint":endpoint,"project":project_data,"includes_history":history}),
    )?;
    let by_id: HashMap<_, _> = entries.iter().map(|e| (e.id, e)).collect();
    let replaced: HashSet<_> = entries
        .iter()
        .filter_map(|e| e.supersedes_memory_id)
        .collect();
    let mut keys = HashSet::new();
    for entry in &entries {
        if replaced.contains(&entry.id) {
            if history {
                write_entry(&temp.0.join("history").join(entry.id.to_string()), entry)?;
            }
        } else {
            let key = lineage_root(entry, &by_id)?;
            ensure!(
                keys.insert(key),
                "memory has competing current revisions; reconcile them before checkout"
            );
            write_entry(&temp.0.join("current").join(key.to_string()), entry)?;
        }
    }
    fs::write(
        temp.0.join("README.md"),
        "# Morrows native memory worktree\n\nUse git status/diff/log/commit, rg/grep, and sed or any editor. Current bodies are in current/, optional retained versions in history/. Source metadata is preserved; edit title.txt and content.md/content.json. Local Git commits do not publish to Morrows. Use morrows memory publish with an authorized source task, context revision, evidence and a retry key. A refresh imports server state and uses native git merge; conflicts remain for you to resolve. Local file deletion is not a server deletion. This SQL bridge publishes one document at a time, not an atomic multi-document Git push.\n",
    )?;
    let commit = import_commit(&bare, &temp.0, &import_ref, old.as_deref(), project)?;
    if dir.exists() {
        git(&dir,&["merge","--no-edit",&commit]).context("native Git merge stopped; inspect git status/diff, resolve and commit or use git merge --abort")?;
    } else {
        git(
            &bare,
            &["update-ref", &format!("refs/heads/{branch}"), &commit, ""],
        )?;
        git(
            &bare,
            &[
                "worktree",
                "add",
                dir.to_str().context("UTF-8 path")?,
                &branch,
            ],
        )?;
    }
    Ok(dir)
}

fn lineage_root(entry: &MemoryEntry, by_id: &HashMap<Uuid, &MemoryEntry>) -> Result<Uuid> {
    let mut current = entry;
    let mut seen = HashSet::new();
    while let Some(parent) = current.supersedes_memory_id {
        ensure!(seen.insert(current.id), "memory lineage cycle");
        current = by_id.get(&parent).context(
            "incomplete memory lineage; retry import rather than rename or lose history",
        )?;
        ensure!(
            current.scope_type == entry.scope_type && current.project_id == entry.project_id,
            "cross-scope memory lineage"
        );
    }
    Ok(current.id)
}

/// Git owns the index, tree, commit graph and CAS ref lock. Each importer uses
/// a private temporary index, so independent projects never share an index or
/// accidentally gain a parent from another project's history.
fn import_commit(
    bare: &Path,
    work: &Path,
    reference: &str,
    old: Option<&str>,
    project: Uuid,
) -> Result<String> {
    let index = work.with_extension("index");
    let result = (|| -> Result<String> {
        let mut add = git_process(bare);
        add.env("GIT_WORK_TREE", work)
            .env("GIT_INDEX_FILE", &index)
            .args(["add", "--all"]);
        output(add, None)?;
        let mut tree = git_process(bare);
        tree.env("GIT_INDEX_FILE", &index).arg("write-tree");
        let tree = output(tree, None)?.trim().to_owned();
        if let Some(old) = old
            && git(bare, &["rev-parse", &format!("{old}^{{tree}}")])? == tree
        {
            return Ok(old.into());
        }
        let mut commit = git_process(bare);
        commit.args(["commit-tree", &tree]);
        if let Some(old) = old {
            commit.args(["-p", old]);
        }
        let id = output(commit, Some(&format!("Import Morrows project {project}\n")))?
            .trim()
            .to_owned();
        git(bare, &["update-ref", reference, &id, old.unwrap_or("")])?;
        Ok(id)
    })();
    if index.exists() {
        fs::remove_file(index)?;
    }
    result
}

fn write_entry(dir: &Path, entry: &MemoryEntry) -> Result<()> {
    fs::create_dir_all(dir)?;
    fs::write(dir.join("title.txt"), &entry.title)?;
    let mut metadata = json!(entry);
    metadata.as_object_mut().unwrap().remove("content");
    write_json(&dir.join("metadata.json"), &metadata)?;
    match entry.content.as_str() {
        Some(text) => fs::write(dir.join("content.md"), text)?,
        None => write_json(&dir.join("content.json"), &entry.content)?,
    }
    Ok(())
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    // Stable metadata ordering keeps native Git diffs and merges about content,
    // independent of the serializer/features used by an MCP client.
    let mut value = value.clone();
    value.sort_all_objects();
    fs::write(path, serde_json::to_string_pretty(&value)? + "\n")?;
    Ok(())
}

fn show(dir: &Path, commit: &str, path: &str) -> Result<String> {
    let mode = git(
        dir,
        &["ls-tree", "--format=%(objectmode)", commit, "--", path],
    )?;
    ensure!(
        matches!(mode.as_str(), "100644" | "100755"),
        "{path} must be a committed regular text file"
    );
    // Cat-file reads the literal blob; textconv, filters and external diff are
    // intentionally not involved in the payload sent to the server.
    output(
        {
            let mut c = git_process(dir);
            c.args(["cat-file", "blob", &format!("{commit}:{path}")]);
            c
        },
        None,
    )
}
fn show_json(dir: &Path, commit: &str, path: &str) -> Result<Value> {
    serde_json::from_str(&show(dir, commit, path)?)
        .with_context(|| format!("parse committed {path}"))
}
fn optional_ref(dir: &Path, reference: &str) -> Result<Option<String>> {
    let result = git_process(dir)
        .args(["show-ref", "--verify", "--quiet", reference])
        .status()?;
    match result.code() {
        Some(0) => Ok(Some(git(dir, &["rev-parse", "--verify", reference])?)),
        Some(1) => Ok(None),
        _ => bail!("cannot read Git ref {reference}"),
    }
}
fn git_process(dir: &Path) -> Process {
    let mut command = Process::new("git");
    command
        .current_dir(dir)
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.autocrlf=false",
            "-c",
            "core.attributesFile=/dev/null",
        ])
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_NAMESPACE")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "Morrows import")
        .env("GIT_AUTHOR_EMAIL", "morrows@localhost")
        .env("GIT_COMMITTER_NAME", "Morrows import")
        .env("GIT_COMMITTER_EMAIL", "morrows@localhost");
    command
}
fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let mut command = git_process(dir);
    command.args(args);
    Ok(output(command, None)?.trim().to_owned())
}
fn output(mut command: Process, input: Option<&str>) -> Result<String> {
    command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    let mut child = command.spawn().context("Git must be installed")?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .context("Git stdin")?
            .write_all(input.as_bytes())?;
    }
    let result = child.wait_with_output()?;
    ensure!(
        result.status.success(),
        "Git failed: {}{}",
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).context("Git output must be UTF-8")
}

struct Temp(PathBuf);
impl Temp {
    fn under(root: &Path) -> Result<Self> {
        let path = root.join(format!(".import-{}", Uuid::new_v4()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(project: Uuid, content: Value) -> MemoryEntry {
        serde_json::from_value(json!({"id":Uuid::new_v4(),"scope_type":"project","project_id":project,
            "title":"知识","content":content,"source_kind":"test","source_ref":"fixture://original",
            "visibility":"shared","created_at":"2026-09-26T00:00:00Z","updated_at":"2026-09-26T00:00:00Z",
            "provenance":{"verification_status":"unverified"}})).unwrap()
    }
    fn import(
        root: &Path,
        project: Uuid,
        entries: Vec<MemoryEntry>,
        refresh: bool,
    ) -> Result<PathBuf> {
        checkout(
            root,
            project,
            "http://localhost/mcp",
            json!({"id":project}),
            entries,
            true,
            refresh,
        )
    }
    #[test]
    fn projects_use_one_object_store_but_independent_histories_and_full_text() {
        let temp = Temp::under(&std::env::temp_dir()).unwrap();
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        let original = entry(a, json!("重复\n重复\n\n最后没有换行"));
        let adir = import(&temp.0, a, vec![original.clone()], false).unwrap();
        let bdir = import(
            &temp.0,
            b,
            vec![entry(b, json!({"retain":[true,null,"原文"]}))],
            false,
        )
        .unwrap();
        assert_eq!(
            git(&adir, &["rev-parse", "--git-common-dir"]).unwrap(),
            git(&bdir, &["rev-parse", "--git-common-dir"]).unwrap()
        );
        let bhead = git(&bdir, &["rev-parse", "HEAD"]).unwrap();
        let aroot = git(&adir, &["rev-list", "--max-parents=0", "HEAD"]).unwrap();
        assert_ne!(
            aroot,
            git(&bdir, &["rev-list", "--max-parents=0", "HEAD"]).unwrap()
        );
        assert_eq!(
            show(
                &adir,
                "HEAD",
                &format!("current/{}/content.md", original.id)
            )
            .unwrap(),
            original.content.as_str().unwrap()
        );
        let mut next = entry(a, json!("修正\n未删除未知信息"));
        next.supersedes_memory_id = Some(original.id);
        import(&temp.0, a, vec![next.clone(), original.clone()], true).unwrap();
        assert_eq!(git(&bdir, &["rev-parse", "HEAD"]).unwrap(), bhead);
        assert_eq!(
            show_json(
                &adir,
                "HEAD",
                &format!("current/{}/metadata.json", original.id)
            )
            .unwrap()["id"],
            json!(next.id)
        );
        assert_eq!(
            show(
                &adir,
                "HEAD",
                &format!("history/{}/content.md", original.id)
            )
            .unwrap(),
            original.content.as_str().unwrap()
        );
        let head = git(&adir, &["rev-parse", "HEAD"]).unwrap();
        import(&temp.0, a, vec![next, original], true).unwrap();
        assert_eq!(git(&adir, &["rev-parse", "HEAD"]).unwrap(), head);
    }
    #[test]
    fn refresh_preserves_dirty_files_and_uses_native_merge_conflicts() {
        let temp = Temp::under(&std::env::temp_dir()).unwrap();
        let project = Uuid::new_v4();
        let original = entry(project, json!("baseline\n"));
        let dir = import(&temp.0, project, vec![original.clone()], false).unwrap();
        let path = dir.join(format!("current/{}/content.md", original.id));
        fs::write(&path, "local edit\n").unwrap();
        let mut remote = entry(project, json!("remote edit\n"));
        remote.supersedes_memory_id = Some(original.id);
        assert!(
            import(
                &temp.0,
                project,
                vec![remote.clone(), original.clone()],
                true
            )
            .is_err()
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "local edit\n");
        git(&dir, &["add", "--all"]).unwrap();
        git(&dir, &["commit", "-m", "Local edit"]).unwrap();
        let error = import(&temp.0, project, vec![remote, original], true).unwrap_err();
        assert!(error.to_string().contains("native Git merge stopped"));
        assert!(
            git(&dir, &["status", "--porcelain"])
                .unwrap()
                .contains("UU")
        );
        git(&dir, &["merge", "--abort"]).unwrap();
        assert_eq!(fs::read_to_string(path).unwrap(), "local edit\n");
    }
}
