//! Git owns project versions; SQLite serializes application writers and stores a
//! recoverable intent before any Git mutation. The two stores are NOT atomic.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    process::{Command, Stdio},
};

const ENTRY_COLUMNS: &[&str] = &[
    "id",
    "scope_type",
    "project_id",
    "agent_instance_id",
    "task_id",
    "title",
    "content_json",
    "source_kind",
    "source_ref",
    "visibility",
    "supersedes_memory_id",
    "created_at",
    "updated_at",
    "provenance_json",
];
const PUBLICATION_COLUMNS: &[&str] = &[
    "agent_instance_id",
    "idempotency_key",
    "input_json",
    "memory_id",
];

/// Raw SQL text is retained, including timestamp spelling and embedded JSON.
/// No serialization through MemoryEntry can normalize the migration evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub(super) struct Snapshot {
    project: String,
    entries: Vec<Value>,
    publications: Vec<Value>,
}

async fn records(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    columns: &[&str],
    filter: &str,
    project: &str,
) -> Result<Vec<Value>, DomainError> {
    let fields = columns
        .iter()
        .map(|c| format!("'{c}',{c}"))
        .collect::<Vec<_>>()
        .join(",");
    let rows: Vec<String> = sqlx::query_scalar(&format!(
        "SELECT json_object({fields}) FROM {table} WHERE {filter} ORDER BY 1"
    ))
    .bind(project)
    .fetch_all(conn)
    .await
    .map_err(storage)?;
    rows.iter()
        .map(|r| serde_json::from_str(r).map_err(storage))
        .collect()
}

pub(super) async fn snapshot(
    conn: &mut sqlx::SqliteConnection,
    project: &str,
) -> Result<Snapshot, DomainError> {
    Ok(Snapshot {
        project: project.into(),
        entries: records(conn, "memory_entries", ENTRY_COLUMNS, "scope_type='project' AND visibility='shared' AND project_id=?", project).await?,
        publications: records(conn, "memory_publications", PUBLICATION_COLUMNS, "memory_id IN (SELECT id FROM memory_entries WHERE scope_type='project' AND visibility='shared' AND project_id=?)", project).await?,
    })
}

impl Store {
    fn git_memory(&self, args: &[&str], input: &[u8]) -> Result<String, DomainError> {
        let mut child = Command::new("git")
            .arg("--git-dir")
            .arg(&self.git_memory_path)
            .args(args)
            .env("GIT_AUTHOR_NAME", "Morrows")
            .env("GIT_AUTHOR_EMAIL", "memory@morrows.local")
            .env("GIT_COMMITTER_NAME", "Morrows")
            .env("GIT_COMMITTER_EMAIL", "memory@morrows.local")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(storage)?;
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input)
            .map_err(storage)?;
        let output = child.wait_with_output().map_err(storage)?;
        if !output.status.success() {
            return Err(DomainError::Conflict(format!(
                "Git memory: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        String::from_utf8(output.stdout).map_err(storage)
    }

    fn memory_ref(project: &str) -> String {
        format!("refs/heads/projects/{project}/main")
    }

    fn git_head(&self, project: &str) -> Result<Option<String>, DomainError> {
        if !self.git_memory_path.exists() {
            return Ok(None);
        }
        let refs = self.git_memory(
            &[
                "for-each-ref",
                "--format=%(objectname)",
                &Self::memory_ref(project),
            ],
            b"",
        )?;
        Ok(if refs.trim().is_empty() {
            None
        } else {
            Some(refs.trim().into())
        })
    }

    fn read_snapshot(&self, head: &str, project: &str) -> Result<Snapshot, DomainError> {
        let snapshot: Snapshot = serde_json::from_str(
            &self.git_memory(&["show", &format!("{head}:snapshot.json")], b"")?,
        )
        .map_err(storage)?;
        if snapshot.project != project {
            return Err(DomainError::Conflict(
                "Git snapshot belongs to another project".into(),
            ));
        }
        Ok(snapshot)
    }

    /// Build a tree without a shared index or worktree. Every retained version
    /// has a native text blob as well as the lossless migration manifest.
    fn commit_snapshot(
        &self,
        snapshot: &Snapshot,
        base: Option<&str>,
        operation: &str,
    ) -> Result<String, DomainError> {
        self.git_memory(&["init", "--bare"], b"")?;
        let mut tree = String::new();
        let mut blob = |name: &str, body: &[u8]| -> Result<(), DomainError> {
            let oid = self.git_memory(&["hash-object", "-w", "--stdin"], body)?;
            tree.push_str(&format!("100644 blob {}\t{name}\n", oid.trim()));
            Ok(())
        };
        blob(
            "snapshot.json",
            &serde_json::to_vec_pretty(snapshot).map_err(storage)?,
        )?;
        for entry in &snapshot.entries {
            let id = entry["id"]
                .as_str()
                .ok_or_else(|| DomainError::Storage("missing memory UUID".into()))?;
            Uuid::parse_str(id).map_err(storage)?;
            let content: Value =
                serde_json::from_str(entry["content_json"].as_str().unwrap()).map_err(storage)?;
            let text = content
                .as_str()
                .map(str::to_owned)
                .unwrap_or(serde_json::to_string_pretty(&content).map_err(storage)?);
            blob(
                &format!("{id}.{}", if content.is_string() { "md" } else { "json" }),
                text.as_bytes(),
            )?;
        }
        let tree = self.git_memory(&["mktree"], tree.as_bytes())?;
        let mut args = vec!["commit-tree", tree.trim()];
        if let Some(base) = base {
            args.extend(["-p", base]);
        }
        let commit = self.git_memory(
            &args,
            format!(
                "Project {} memory operation {operation}\n",
                snapshot.project
            )
            .as_bytes(),
        )?;
        let commit = commit.trim().to_owned();
        // Retain prepared objects even across aggressive GC and process death.
        self.git_memory(
            &[
                "update-ref",
                &format!("refs/morrows/operations/{operation}"),
                &commit,
            ],
            b"",
        )?;
        Ok(commit)
    }

    pub async fn project_memory_head(&self, project: Id) -> Result<Option<String>, DomainError> {
        let backend: Option<String> =
            sqlx::query_scalar("SELECT backend FROM memory_git_domains WHERE domain=?")
                .bind(project.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(storage)?;
        if backend.as_deref() == Some("git") {
            self.git_head(&project.to_string())
        } else {
            Ok(None)
        }
    }

    pub(super) async fn project_memory_head_conn(
        &self,
        conn: &mut sqlx::SqliteConnection,
        project: Id,
    ) -> Result<Option<String>, DomainError> {
        let indexed: Option<String> =
            sqlx::query_scalar("SELECT indexed_commit FROM memory_git_domains WHERE domain=?")
                .bind(project.to_string())
                .fetch_optional(&mut *conn)
                .await
                .map_err(storage)?
                .flatten();
        let head = self.git_head(&project.to_string())?;
        if head != indexed || head.is_none() {
            return Err(DomainError::Conflict(
                "Git projection requires reconciliation".into(),
            ));
        }
        if snapshot(conn, &project.to_string()).await?
            != self.read_snapshot(head.as_deref().unwrap(), &project.to_string())?
        {
            return Err(DomainError::Conflict(
                "SQL projection differs from Git".into(),
            ));
        }
        Ok(head)
    }

    /// SQL remains authoritative. Preparing the durable intent precedes even
    /// object creation; restarting can finish the mirror without inventing data.
    pub async fn mirror_project_memory(&self, project: Id) -> Result<String, DomainError> {
        self.get_project(project).await?;
        self.reconcile_git_memory().await?;
        let project = project.to_string();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        sqlx::query("INSERT OR IGNORE INTO memory_git_domains(domain,backend,repository,ref_name,updated_at) VALUES(?,'sql',?,?,?)")
            .bind(&project).bind(self.git_memory_path.to_string_lossy().as_ref()).bind(Self::memory_ref(&project)).bind(Utc::now().to_rfc3339()).execute(&mut *tx).await.map_err(storage)?;
        if Self::git_backend(&mut tx, &project).await? {
            return Err(DomainError::Conflict("project already uses Git".into()));
        }
        let data = snapshot(&mut tx, &project).await?;
        let base = self.git_head(&project)?;
        let op = self
            .prepare_memory_operation(
                &mut tx,
                &project,
                "mirror",
                None,
                None,
                "{}",
                &data,
                base.as_deref(),
            )
            .await?;
        tx.commit().await.map_err(storage)?;
        self.reconcile_git_memory().await?;
        let head: Option<String> = sqlx::query_scalar(
            "SELECT o.result_commit FROM memory_git_operations o JOIN memory_git_domains d ON d.domain=o.domain WHERE o.id=? AND o.state='indexed' AND d.verified_commit=o.result_commit",
        )
        .bind(op)
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .flatten();
        head.ok_or_else(|| DomainError::Conflict("mirror operation conflicted".into()))
    }

    /// The same writer lock protects snapshot comparison and cutover. A mirror
    /// that was verified before a subsequent SQL write cannot pass this gate.
    pub async fn cutover_project_memory(
        &self,
        project: Id,
        verified_commit: &str,
    ) -> Result<(), DomainError> {
        self.reconcile_git_memory().await?;
        let project = project.to_string();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_git_operations WHERE domain=? AND state='prepared')").bind(&project).fetch_one(&mut *tx).await.map_err(storage)?;
        if pending {
            return Err(DomainError::Conflict(
                "pending memory operation; reconcile before cutover".into(),
            ));
        }
        let verified: Option<String> =
            sqlx::query_scalar("SELECT verified_commit FROM memory_git_domains WHERE domain=?")
                .bind(&project)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .flatten();
        if verified.as_deref() != Some(verified_commit)
            || self.git_head(&project)?.as_deref() != Some(verified_commit)
            || snapshot(&mut tx, &project).await?
                != self.read_snapshot(verified_commit, &project)?
        {
            return Err(DomainError::Conflict(
                "cutover requires exact current SQL equivalence with the verified mirror head"
                    .into(),
            ));
        }
        let data = self.read_snapshot(verified_commit, &project)?;
        let ids: std::collections::HashSet<&str> = data
            .entries
            .iter()
            .filter_map(|entry| entry["id"].as_str())
            .collect();
        if data.entries.iter().any(|entry| {
            entry["supersedes_memory_id"]
                .as_str()
                .is_some_and(|id| !ids.contains(id))
        }) {
            return Err(DomainError::Conflict("shared history has a missing or private ancestor; reconcile visibility before cutover".into()));
        }
        sqlx::query("UPDATE memory_git_domains SET backend='git',indexed_commit=?,updated_at=? WHERE domain=?")
            .bind(verified_commit).bind(Utc::now().to_rfc3339()).bind(project).execute(&mut *tx).await.map_err(storage)?;
        tx.commit().await.map_err(storage)
    }

    pub(super) async fn git_backend(
        conn: &mut sqlx::SqliteConnection,
        project: &str,
    ) -> Result<bool, DomainError> {
        let backend: Option<String> =
            sqlx::query_scalar("SELECT backend FROM memory_git_domains WHERE domain=?")
                .bind(project)
                .fetch_optional(conn)
                .await
                .map_err(storage)?;
        Ok(backend.as_deref() == Some("git"))
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn prepare_memory_operation(
        &self,
        conn: &mut sqlx::SqliteConnection,
        project: &str,
        kind: &str,
        actor: Option<Id>,
        key: Option<&str>,
        request: &str,
        data: &Snapshot,
        base: Option<&str>,
    ) -> Result<String, DomainError> {
        let id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO memory_git_operations(id,domain,kind,actor_id,retry_key,request_json,payload_json,base_commit,state,created_at) VALUES(?,?,?,?,?,?,?,?,'prepared',?)")
            .bind(&id).bind(project).bind(kind).bind(actor.map(|id| id.to_string())).bind(key).bind(request).bind(serde_json::to_string(data).map_err(storage)?).bind(base).bind(Utc::now().to_rfc3339()).execute(conn).await.map_err(storage)?;
        Ok(id)
    }

    /// Each operation holds the application writer lock while comparing the real
    /// Git ref and indexing. After a crash, result_commit (or its retained ref)
    /// identifies the exact CAS result. Unexpected ref movement becomes conflict;
    /// recovery never rewrites that ref. Projection updates are idempotent.
    pub async fn reconcile_git_memory(&self) -> Result<(), DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let operations = sqlx::query(
            "SELECT * FROM memory_git_operations WHERE state='prepared' ORDER BY created_at,id",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(storage)?;
        for row in operations {
            let id: String = row.try_get("id").map_err(storage)?;
            let project: String = row.try_get("domain").map_err(storage)?;
            let kind: String = row.try_get("kind").map_err(storage)?;
            let base: Option<String> = row.try_get("base_commit").map_err(storage)?;
            let payload: String = row.try_get("payload_json").map_err(storage)?;
            let data: Snapshot = serde_json::from_str(&payload).map_err(storage)?;
            let retained = if self.git_memory_path.exists() {
                self.git_memory(
                    &[
                        "for-each-ref",
                        "--format=%(objectname)",
                        &format!("refs/morrows/operations/{id}"),
                    ],
                    b"",
                )?
            } else {
                String::new()
            };
            let result = if retained.trim().is_empty() {
                self.commit_snapshot(&data, base.as_deref(), &id)?
            } else {
                retained.trim().to_owned()
            };
            if self.read_snapshot(&result, &project)? != data {
                return Err(DomainError::Conflict(
                    "prepared Git snapshot differs from durable operation".into(),
                ));
            }
            let head = self.git_head(&project)?;
            let matches = if head.as_deref() == Some(&result) {
                true
            } else if head == base {
                self.git_memory(
                    &[
                        "update-ref",
                        &Self::memory_ref(&project),
                        &result,
                        base.as_deref()
                            .unwrap_or("0000000000000000000000000000000000000000"),
                    ],
                    b"",
                )
                .is_ok()
            } else {
                false
            };
            if !matches {
                sqlx::query(
                    "UPDATE memory_git_operations SET state='conflict',result_commit=? WHERE id=?",
                )
                .bind(&result)
                .bind(&id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
                continue;
            }
            #[cfg(test)]
            if self
                .fail_after_memory_cas
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(DomainError::Storage(
                    "simulated process death after Git CAS".into(),
                ));
            }
            if kind == "publication" {
                index_snapshot(&mut tx, &data).await?;
                let actor: String = row.try_get("actor_id").map_err(storage)?;
                let request: String = row.try_get("request_json").map_err(storage)?;
                let request: morrows_core::PublishProjectMemory =
                    serde_json::from_str(&request).map_err(storage)?;
                append_event_tx(
                    &mut tx,
                    "agent_instance",
                    &actor,
                    "task",
                    request.task_id,
                    "memory.project_published",
                    json!({"project_id":project,"operation_id":id,"memory_head":result}),
                    None,
                )
                .await?;

                sqlx::query(
                    "UPDATE memory_git_domains SET indexed_commit=?,updated_at=? WHERE domain=?",
                )
                .bind(&result)
                .bind(Utc::now().to_rfc3339())
                .bind(&project)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            } else if snapshot(&mut tx, &project).await? == data {
                sqlx::query("UPDATE memory_git_domains SET verified_commit=?,verified_count=?,updated_at=? WHERE domain=? AND backend='sql'").bind(&result).bind(data.entries.len() as i64).bind(Utc::now().to_rfc3339()).bind(&project).execute(&mut *tx).await.map_err(storage)?;
            }
            sqlx::query(
                "UPDATE memory_git_operations SET state='indexed',result_commit=? WHERE id=?",
            )
            .bind(result)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }
        tx.commit().await.map_err(storage)
    }

    /// Rebuild missing/corrupted index rows from the known authoritative head.
    /// Unexpected ref movement or extra SQL rows remain explicit conflicts; this
    /// repair cannot approve an unknown Git commit or delete historical evidence.
    pub async fn rebuild_project_memory_projection(&self, project: Id) -> Result<(), DomainError> {
        self.reconcile_git_memory().await?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let project = project.to_string();
        if !Self::git_backend(&mut tx, &project).await? {
            return Err(DomainError::Conflict("project still uses SQL".into()));
        }
        let indexed: Option<String> =
            sqlx::query_scalar("SELECT indexed_commit FROM memory_git_domains WHERE domain=?")
                .bind(&project)
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?;
        let head = self
            .git_head(&project)?
            .ok_or_else(|| DomainError::Conflict("Git head missing".into()))?;
        if indexed.as_deref() != Some(&head) {
            return Err(DomainError::Conflict(
                "unexpected Git head requires investigation".into(),
            ));
        }
        let data = self.read_snapshot(&head, &project)?;
        index_snapshot(&mut tx, &data).await?;
        tx.commit().await.map_err(storage)
    }

    /// Readers hold this transaction through their SELECT. A completed recovery
    /// followed by an unlocked SELECT would leave a race with the next CAS.
    pub(super) async fn memory_read_transaction(
        &self,
    ) -> Result<sqlx::Transaction<'_, sqlx::Sqlite>, DomainError> {
        self.reconcile_git_memory().await?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let domains =
            sqlx::query("SELECT domain,indexed_commit FROM memory_git_domains WHERE backend='git'")
                .fetch_all(&mut *tx)
                .await
                .map_err(storage)?;
        for row in domains {
            let project: String = row.try_get("domain").map_err(storage)?;
            let indexed: Option<String> = row.try_get("indexed_commit").map_err(storage)?;
            let head = self
                .git_head(&project)?
                .ok_or_else(|| DomainError::Conflict("authoritative memory ref missing".into()))?;
            if indexed.as_deref() != Some(&head) {
                return Err(DomainError::Conflict("Git memory projection is not indexed to authoritative head; reconcile required".into()));
            }
            // Git is authoritative even if SQL was modified outside the application.
            if snapshot(&mut tx, &project).await? != self.read_snapshot(&head, &project)? {
                return Err(DomainError::Conflict(
                    "Git memory projection differs from authoritative content".into(),
                ));
            }
        }
        Ok(tx)
    }
}

async fn index_snapshot(
    conn: &mut sqlx::SqliteConnection,
    data: &Snapshot,
) -> Result<(), DomainError> {
    // All old versions remain. Insert ancestors first to satisfy supersedes FKs.
    let mut pending = data.entries.clone();
    while !pending.is_empty() {
        let before = pending.len();
        let mut next = Vec::new();
        for entry in pending {
            let owner: Option<(String, Option<String>, String)> = sqlx::query_as(
                "SELECT scope_type,project_id,visibility FROM memory_entries WHERE id=?",
            )
            .bind(entry["id"].as_str())
            .fetch_optional(&mut *conn)
            .await
            .map_err(storage)?;
            if owner.is_some_and(|(scope, project, visibility)| {
                scope != "project"
                    || project.as_deref() != Some(&data.project)
                    || visibility != "shared"
            }) {
                return Err(DomainError::Conflict(
                    "memory UUID belongs to another scope or visibility".into(),
                ));
            }
            if let Some(parent) = entry["supersedes_memory_id"].as_str() {
                let exists: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_entries WHERE id=?)")
                        .bind(parent)
                        .fetch_one(&mut *conn)
                        .await
                        .map_err(storage)?;
                if !exists {
                    next.push(entry);
                    continue;
                }
            }
            insert_record(conn, "memory_entries", ENTRY_COLUMNS, &entry).await?;
        }
        if next.len() == before {
            return Err(DomainError::Conflict(
                "missing or cyclic supersedes history".into(),
            ));
        }
        pending = next;
    }
    for publication in &data.publications {
        insert_record(
            conn,
            "memory_publications",
            PUBLICATION_COLUMNS,
            publication,
        )
        .await?;
    }
    if snapshot(conn, &data.project).await? != *data {
        return Err(DomainError::Conflict(
            "indexed snapshot differs from Git".into(),
        ));
    }
    Ok(())
}

async fn insert_record(
    conn: &mut sqlx::SqliteConnection,
    table: &str,
    columns: &[&str],
    record: &Value,
) -> Result<(), DomainError> {
    let placeholders = vec!["?"; columns.len()].join(",");
    let updates = columns
        .iter()
        .map(|column| format!("{column}=excluded.{column}"))
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "INSERT INTO {table}({}) VALUES({placeholders}) ON CONFLICT DO UPDATE SET {updates}",
        columns.join(",")
    );

    let mut query = sqlx::query(&sql);
    for column in columns {
        query = query.bind(record[*column].as_str());
    }
    query.execute(conn).await.map_err(storage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use morrows_core::{CreateMemoryEntry, PublishProjectMemory};

    async fn project(store: &Store) -> Id {
        store
            .create_project(CreateProject {
                name: "Memory".into(),
                description: "full text".into(),
            })
            .await
            .unwrap()
            .id
    }
    fn memory(project: Id, previous: Option<Id>) -> CreateMemoryEntry {
        serde_json::from_value(json!({"scope_type":"project","project_id":project,"title":"Original title 中文", "content":"line one\n\nline one\nuncertain\n", "source_kind":"import","source_ref":"original:source", "visibility":"shared", "supersedes_memory_id":previous})).unwrap()
    }
    async fn author(store: &Store, project: Id) -> (Id, PublishProjectMemory) {
        let agent = store.register_agent("writer", &[]).await.unwrap().id;
        let task = store
            .create_task(
                serde_json::from_value(
                    json!({"title":"Source", "project_id":project, "state":"ready"}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .claim_task(task.id, agent, "executor", 3600)
            .await
            .unwrap();
        let context = store.create_context_revision(task.id, serde_json::from_value(json!({"goal":"keep evidence", "background":"source", "current_summary":"observed", "constraints":{}})).unwrap()).await.unwrap();
        let input = serde_json::from_value(json!({"task_id":task.id,"context_revision_id":context.id,"expected_project_id":project,"idempotency_key":"retry", "title":"Original", "content":{"complete":"not a summary","unknown":null},"verification_status":"unverified","basis":"actual source"})).unwrap();
        (agent, input)
    }
    async fn sql_snapshot(store: &Store, project: Id) -> Snapshot {
        snapshot(
            &mut *store.pool.acquire().await.unwrap(),
            &project.to_string(),
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn mirror_full_equivalence_independent_roots_and_cutover_gate() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = project(&store).await;
        let b = project(&store).await;
        let first = store.create_memory_entry(memory(a, None)).await.unwrap();
        store
            .create_memory_entry(memory(a, Some(first.id)))
            .await
            .unwrap();
        let (agent, mut input) = author(&store, a).await;
        let artifact = store.create_artifact(input.task_id, agent, serde_json::from_value(json!({"title":"Original evidence", "uri":"file:original-evidence", "kind":"test", "description":"uncertain detail retained"})).unwrap()).await.unwrap();
        input.verification_status = "verified".into();
        input.artifact_ids = vec![artifact.id.to_string()];

        let published = store.publish_project_memory(agent, input).await.unwrap();
        // Noncanonical timestamp spelling, provenance and publication JSON must
        // round-trip byte-for-byte, not just via normalized Rust domain values.
        sqlx::query("UPDATE memory_entries SET created_at='2001-02-03T04:05:06+00:00',updated_at='2002-03-04T05:06:07.120000+00:00' WHERE id=?").bind(first.id.to_string()).execute(&store.pool).await.unwrap();
        let before = sql_snapshot(&store, a).await;
        assert_eq!(before.entries.len(), 3);
        assert_eq!(before.publications.len(), 1);
        assert!(before.entries.iter().any(|v| {
            v["id"] == published.id.to_string()
                && v["provenance_json"]
                    .as_str()
                    .unwrap()
                    .contains("actual source")
        }));
        let a_head = store.mirror_project_memory(a).await.unwrap();
        let b_head = store.mirror_project_memory(b).await.unwrap();
        assert_ne!(a_head, b_head);
        assert_eq!(
            store.read_snapshot(&a_head, &a.to_string()).unwrap(),
            before
        );
        assert_eq!(sql_snapshot(&store, a).await, before);
        for head in [&a_head, &b_head] {
            assert_eq!(
                store
                    .git_memory(&["rev-list", "--count", head], b"")
                    .unwrap()
                    .trim(),
                "1"
            );
        }
        assert!(store.project_memory_head(a).await.unwrap().is_none());
        assert!(store.cutover_project_memory(a, &b_head).await.is_err());
        store.create_memory_entry(memory(a, None)).await.unwrap();
        assert!(store.cutover_project_memory(a, &a_head).await.is_err());
        let refreshed = store.mirror_project_memory(a).await.unwrap();
        assert_ne!(refreshed, a_head);
        assert_eq!(store.git_head(&b.to_string()).unwrap(), Some(b_head));
        store.cutover_project_memory(a, &refreshed).await.unwrap();
        assert_eq!(
            store.get_project(a).await.unwrap().memory_head,
            Some(refreshed.clone())
        );
        assert!(store.create_memory_entry(memory(a, None)).await.is_err());
        assert_eq!(store.git_head(&a.to_string()).unwrap(), Some(refreshed));
        // Retained operation refs and project history survive native Git GC.
        store.git_memory(&["gc", "--prune=now"], b"").unwrap();
        assert_eq!(
            store.read_snapshot(&a_head, &a.to_string()).unwrap(),
            before
        );
        let text = store
            .git_memory(&["show", &format!("{a_head}:{}.md", first.id)], b"")
            .unwrap();
        assert_eq!(text, first.content.as_str().unwrap());
    }

    #[tokio::test]
    async fn git_publication_cas_retry_and_sql_divergence() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = project(&store).await;
        let b = project(&store).await;
        let (agent, mut input) = author(&store, a).await;
        let head = store.mirror_project_memory(a).await.unwrap();
        let b_head = store.mirror_project_memory(b).await.unwrap();
        store.cutover_project_memory(a, &head).await.unwrap();
        assert!(
            store
                .publish_project_memory(agent, input.clone())
                .await
                .is_err()
        );
        input.base_commit = Some(b_head.clone());
        assert!(
            store
                .publish_project_memory(agent, input.clone())
                .await
                .is_err()
        );
        input.base_commit = Some(head.clone());
        let result = store
            .publish_project_memory(agent, input.clone())
            .await
            .unwrap();
        let new_head = store.project_memory_head(a).await.unwrap().unwrap();
        assert_ne!(new_head, head);
        assert_eq!(store.git_head(&b.to_string()).unwrap(), Some(b_head));
        assert_eq!(
            store
                .publish_project_memory(agent, input.clone())
                .await
                .unwrap()
                .id,
            result.id
        );
        let mut changed = input.clone();
        changed.content = json!("different");
        assert!(store.publish_project_memory(agent, changed).await.is_err());
        let mut stale = input.clone();
        stale.idempotency_key = "stale".into();
        assert!(store.publish_project_memory(agent, stale).await.is_err());
        let mut revision = input.clone();
        revision.idempotency_key = "revision".into();
        revision.base_commit = Some(new_head.clone());
        revision.supersedes_memory_id = Some(result.id);
        revision.content = json!("full revision\n");
        let updated = store.publish_project_memory(agent, revision).await.unwrap();
        assert_eq!(updated.supersedes_memory_id, Some(result.id));
        let current = store
            .context_memories_page(None, Some(a), None, false, 100, 0)
            .await
            .unwrap();
        assert_eq!(current.items.len(), 1);
        let history = store
            .context_memories_page(None, Some(a), None, true, 100, 0)
            .await
            .unwrap();
        assert_eq!(history.items.len(), 2);
        assert_eq!(
            store.get_memory_entry(result.id).await.unwrap().content,
            input.content
        );
        assert!(store.create_memory_entry(memory(a, None)).await.is_err());
        // External SQL corruption cannot be served as authoritative memory.
        sqlx::query("UPDATE memory_entries SET title='SQL-only corruption' WHERE id=?")
            .bind(result.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(store.get_memory_entry(result.id).await.is_err());
        assert!(
            store
                .list_memory_entries(Some("project"), Some(a), None, None)
                .await
                .is_err()
        );
        store.rebuild_project_memory_projection(a).await.unwrap();
        assert_eq!(
            store.get_memory_entry(result.id).await.unwrap().title,
            result.title
        );
    }

    #[tokio::test]
    async fn crash_after_cas_reopens_and_indexes_once() {
        let path =
            std::env::temp_dir().join(format!("morrows-memory-recovery-{}.sqlite", Uuid::new_v4()));
        let url = format!("sqlite://{}", path.display());
        let store = Store::connect(&url).await.unwrap();
        let a = project(&store).await;
        let (agent, mut input) = author(&store, a).await;
        let head = store.mirror_project_memory(a).await.unwrap();
        store.cutover_project_memory(a, &head).await.unwrap();
        input.base_commit = Some(head.clone());
        store
            .fail_after_memory_cas
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            store
                .publish_project_memory(agent, input.clone())
                .await
                .is_err()
        );
        assert_ne!(store.git_head(&a.to_string()).unwrap(), Some(head.clone()));
        assert_eq!(sql_snapshot(&store, a).await.entries.len(), 0);
        let pending: i64 =
            sqlx::query_scalar("SELECT count(*) FROM memory_git_operations WHERE state='prepared'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(pending, 1);
        store.pool.close().await;
        let reopened = Store::connect(&url).await.unwrap();
        let result = reopened
            .publish_project_memory(agent, input.clone())
            .await
            .unwrap();
        reopened.reconcile_git_memory().await.unwrap();
        assert_eq!(
            reopened
                .publish_project_memory(agent, input)
                .await
                .unwrap()
                .id,
            result.id
        );
        assert_eq!(sql_snapshot(&reopened, a).await.entries.len(), 1);
        let events: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE event_type='memory.project_published'",
        )
        .fetch_one(&reopened.pool)
        .await
        .unwrap();
        assert_eq!(events, 1);
        reopened.pool.close().await;
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir_all(&reopened.git_memory_path).unwrap();
    }

    #[tokio::test]
    async fn recovery_conflicts_when_ref_moved_after_cas() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = project(&store).await;
        let (agent, mut input) = author(&store, a).await;
        let head = store.mirror_project_memory(a).await.unwrap();
        store.cutover_project_memory(a, &head).await.unwrap();
        input.base_commit = Some(head.clone());
        store
            .fail_after_memory_cas
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            store
                .publish_project_memory(agent, input.clone())
                .await
                .is_err()
        );
        let actual = store.git_head(&a.to_string()).unwrap().unwrap();
        let data = store.read_snapshot(&actual, &a.to_string()).unwrap();
        let unexpected = store
            .commit_snapshot(&data, Some(&actual), &Uuid::new_v4().to_string())
            .unwrap();
        store
            .git_memory(
                &[
                    "update-ref",
                    &Store::memory_ref(&a.to_string()),
                    &unexpected,
                    &actual,
                ],
                b"",
            )
            .unwrap();
        store.reconcile_git_memory().await.unwrap();
        let state: String =
            sqlx::query_scalar("SELECT state FROM memory_git_operations WHERE kind='publication'")
                .fetch_one(&store.pool)
                .await
                .unwrap();
        assert_eq!(state, "conflict");
        assert_eq!(store.git_head(&a.to_string()).unwrap(), Some(unexpected));
        assert_eq!(sql_snapshot(&store, a).await.entries.len(), 0);
        assert!(store.publish_project_memory(agent, input).await.is_err());
        assert!(
            store
                .context_memories_page(None, Some(a), None, false, 100, 0)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn concurrent_writers_have_one_winner_and_retry_original_receipt() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = project(&store).await;
        let (agent, mut input) = author(&store, a).await;
        let head = store.mirror_project_memory(a).await.unwrap();
        store.cutover_project_memory(a, &head).await.unwrap();
        input.base_commit = Some(head);
        let mut other = input.clone();
        other.idempotency_key = "other writer".into();
        let (left, right) = tokio::join!(
            store.publish_project_memory(agent, input.clone()),
            store.publish_project_memory(agent, other.clone())
        );
        assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
        let winning = if left.is_ok() { input } else { other };
        assert!(store.publish_project_memory(agent, winning).await.is_ok());
        assert_eq!(sql_snapshot(&store, a).await.entries.len(), 1);
    }

    #[tokio::test]
    async fn prepared_mirror_recovers_before_any_git_objects_exist() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = project(&store).await;
        store.create_memory_entry(memory(a, None)).await.unwrap();
        let mut tx = store.pool.begin_with("BEGIN IMMEDIATE").await.unwrap();
        sqlx::query("INSERT INTO memory_git_domains(domain,backend,repository,ref_name,updated_at) VALUES(?,'sql',?,?,?)")
            .bind(a.to_string()).bind(store.git_memory_path.to_string_lossy().as_ref()).bind(Store::memory_ref(&a.to_string())).bind(Utc::now().to_rfc3339()).execute(&mut *tx).await.unwrap();
        let data = snapshot(&mut tx, &a.to_string()).await.unwrap();
        store
            .prepare_memory_operation(
                &mut tx,
                &a.to_string(),
                "mirror",
                None,
                None,
                "{}",
                &data,
                None,
            )
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(!store.git_memory_path.exists());
        store.reconcile_git_memory().await.unwrap();
        let head = store.git_head(&a.to_string()).unwrap().unwrap();
        assert_eq!(store.read_snapshot(&head, &a.to_string()).unwrap(), data);
        store.cutover_project_memory(a, &head).await.unwrap();
    }

    #[tokio::test]
    async fn cutover_does_not_leak_a_private_ancestor() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = project(&store).await;
        let mut private = memory(a, None);
        private.visibility = "private".into();
        let private = store.create_memory_entry(private).await.unwrap();
        store
            .create_memory_entry(memory(a, Some(private.id)))
            .await
            .unwrap();
        let head = store.mirror_project_memory(a).await.unwrap();
        let data = store.read_snapshot(&head, &a.to_string()).unwrap();
        assert_eq!(data.entries.len(), 1);
        assert!(store.cutover_project_memory(a, &head).await.is_err());
        assert!(store.project_memory_head(a).await.unwrap().is_none());
    }
}
