use super::*;
use morrows_core::{CreateMemoryEntry, MemoryEntry};

impl Store {
    pub async fn ensure_task_write_access(
        &self,
        task_id: Id,
        agent_id: Id,
    ) -> Result<(), DomainError> {
        let mut conn = self.pool.acquire().await.map_err(storage)?;
        task_writer_conn(&mut conn, task_id, agent_id)
            .await
            .map(|_| ())
    }

    /// Bind project and author to the authorized source task inside the writer
    /// transaction. Retries return the original publication; revisions append a
    /// new entry and reject stale supersession rather than forking shared truth.
    pub async fn publish_project_memory(
        &self,
        agent_id: Id,
        input: morrows_core::PublishProjectMemory,
    ) -> Result<MemoryEntry, DomainError> {
        self.reconcile_git_memory().await?;
        if input.idempotency_key.trim().is_empty() || input.idempotency_key.len() > 128 {
            return Err(DomainError::InvalidInput(
                "idempotency_key must contain 1..128 bytes".into(),
            ));
        }
        if input.basis.trim().is_empty()
            || input.content.is_null()
            || !matches!(
                input.verification_status.as_str(),
                "reported" | "verified" | "hypothesis" | "unverified"
            )
        {
            return Err(DomainError::InvalidInput("provide content, basis and verification_status (reported/verified/hypothesis/unverified)".into()));
        }
        if input.verification_status == "verified"
            && input.artifact_ids.is_empty()
            && input.decision_ids.is_empty()
        {
            return Err(DomainError::InvalidInput(
                "verified findings require task artifact or decision references".into(),
            ));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let task = task_writer_conn(&mut tx, input.task_id, agent_id).await?;
        // The durable prepare/CAS gap must also reserve caller-selected UUIDs
        // against publications in other projects. Retry will reconcile first.
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM memory_git_operations WHERE state='prepared')",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if pending {
            return Err(DomainError::Conflict(
                "memory operation pending; retry after reconciliation".into(),
            ));
        }
        let input_json = serde_json::to_string(&input).map_err(storage)?;
        if let Some(row) = sqlx::query(
            "SELECT request_json,state FROM memory_git_operations WHERE actor_id=? AND retry_key=?",
        )
        .bind(agent_id.to_string())
        .bind(&input.idempotency_key)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        {
            let saved: String = row.try_get("request_json").map_err(storage)?;
            let state: String = row.try_get("state").map_err(storage)?;
            if saved != input_json || state != "indexed" {
                return Err(DomainError::Conflict("Git publication retry differs or operation conflicted; reconcile before retrying".into()));
            }
        }
        if let Some(row) = sqlx::query("SELECT memory_id,input_json FROM memory_publications WHERE agent_instance_id=? AND idempotency_key=?")
            .bind(agent_id.to_string()).bind(&input.idempotency_key).fetch_optional(&mut *tx).await.map_err(storage)? {
            let saved: String = row.try_get("input_json").map_err(storage)?;
            if saved != input_json { return Err(DomainError::Conflict("idempotency_key already used with different content".into())); }
            let id = parse_id(row.try_get("memory_id").map_err(storage)?)?;
            tx.commit().await.map_err(storage)?;
            return self.get_memory_entry(id).await;
        }
        let project_id = task.project_id.ok_or_else(|| {
            DomainError::Conflict(
                "task has no project; control plane must set its project first".into(),
            )
        })?;
        if input
            .expected_project_id
            .is_some_and(|expected| expected != project_id)
        {
            return Err(DomainError::Conflict("task project changed; reconcile the source task and target project before publishing".into()));
        }
        let git_backend = Self::git_backend(&mut tx, &project_id.to_string()).await?;
        if git_backend
            && (input.base_commit.is_none()
                || self.project_memory_head_conn(&mut tx, project_id).await? != input.base_commit)
        {
            return Err(DomainError::Conflict(
                "base_commit must match the project head read by the caller".into(),
            ));
        }
        if let Some(id) = input.new_memory_id {
            if input.supersedes_memory_id.is_some() {
                return Err(DomainError::InvalidInput(
                    "new_memory_id is only for a new document, not a revision".into(),
                ));
            }
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_entries WHERE id=?)")
                    .bind(id.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage)?;
            if exists {
                return Err(DomainError::Conflict("new_memory_id is already in use; reuse the original publication key or choose a new document UUID".into()));
            }
        }
        if task.current_context_revision_id != Some(input.context_revision_id) {
            return Err(DomainError::Conflict(
                "context changed; read task_context before publishing project knowledge".into(),
            ));
        }
        for (table, ids) in [
            ("artifacts", &input.artifact_ids),
            ("decisions", &input.decision_ids),
        ] {
            for id in ids {
                let valid: bool = sqlx::query_scalar(&format!(
                    "SELECT EXISTS(SELECT 1 FROM {table} WHERE id=? AND task_id=?)"
                ))
                .bind(id)
                .bind(task.id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?;
                if !valid {
                    return Err(DomainError::InvalidInput(format!(
                        "{table} reference {id} does not belong to the source task"
                    )));
                }
            }
        }
        if let Some(previous) = input.supersedes_memory_id {
            let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM memory_entries m WHERE id=? AND scope_type='project' AND project_id=? AND visibility='shared' AND NOT EXISTS(SELECT 1 FROM memory_entries n WHERE n.supersedes_memory_id=m.id AND n.visibility='shared'))")
                .bind(previous.to_string()).bind(project_id.to_string()).fetch_one(&mut *tx).await.map_err(storage)?;
            if !valid {
                return Err(DomainError::Conflict("supersedes_memory_id must be current shared memory in the source task's project; read project_get to reconcile".into()));
            }
        }
        sqlx::query("SAVEPOINT memory_payload")
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        let entry = CreateMemoryEntry {
            scope_type: "project".into(),
            project_id: Some(project_id),
            agent_instance_id: None,
            task_id: None,
            title: input.title,
            content: input.content,
            source_kind: "agent_task".into(),
            source_ref: Some(format!("morrows:task:{}", task.id)),
            visibility: "shared".into(),
            supersedes_memory_id: input.supersedes_memory_id,
        };
        validate_memory_input(&entry)?;
        let id = insert_memory_entry_conn(&mut tx, &entry, input.new_memory_id).await?;
        let provenance = json!({"agent_instance_id":agent_id,"task_id":task.id,"context_revision_id":input.context_revision_id,
            "artifact_ids":input.artifact_ids,"decision_ids":input.decision_ids,"basis":input.basis,
            "verification_status":input.verification_status,"recorded_at":Utc::now(),"server_verified":false});
        sqlx::query("UPDATE memory_entries SET provenance_json=? WHERE id=?")
            .bind(provenance.to_string())
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        sqlx::query("INSERT INTO memory_publications(agent_instance_id,idempotency_key,input_json,memory_id) VALUES(?,?,?,?)")
            .bind(agent_id.to_string()).bind(&input.idempotency_key).bind(&input_json).bind(id.to_string()).execute(&mut *tx).await.map_err(storage)?;
        if git_backend {
            let payload = super::git_memory::snapshot(&mut tx, &project_id.to_string()).await?;
            // Validation used the normal SQL constraints in a savepoint. Roll it
            // back: only the durable intent may become visible before Git CAS.
            sqlx::query("ROLLBACK TO memory_payload")
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            sqlx::query("RELEASE memory_payload")
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            let operation = self
                .prepare_memory_operation(
                    &mut tx,
                    &project_id.to_string(),
                    "publication",
                    Some(agent_id),
                    Some(&input.idempotency_key),
                    &input_json,
                    &payload,
                    input.base_commit.as_deref(),
                )
                .await?;
            tx.commit().await.map_err(storage)?;
            self.reconcile_git_memory().await?;
            let state: String =
                sqlx::query_scalar("SELECT state FROM memory_git_operations WHERE id=?")
                    .bind(operation)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(storage)?;
            if state != "indexed" {
                return Err(DomainError::Conflict(
                    "Git publication CAS conflicted; read the latest project head".into(),
                ));
            }
            return self.get_memory_entry(id).await;
        }
        append_event_tx(&mut tx,"agent_instance",&agent_id.to_string(),"task",task.id,"memory.project_published",
            json!({"memory_id":id,"project_id":project_id,"supersedes_memory_id":entry.supersedes_memory_id}),None).await?;
        tx.commit().await.map_err(storage)?;
        self.get_memory_entry(id).await
    }

    /// Materialize visible knowledge, retaining the caller's private agent memory
    /// but never another agent's. History mode includes superseded entries with
    /// their original provenance; the default excludes only visible replacements,
    /// so a private replacement cannot hide a still-visible shared fact.
    pub async fn context_memories_page(
        &self,
        task_id: Option<Id>,
        project_id: Option<Id>,
        agent_id: Option<Id>,
        include_superseded: bool,
        limit: i64,
        offset: i64,
    ) -> Result<morrows_core::Page<MemoryEntry>, DomainError> {
        super::discovery::validate_page(limit, offset)?;
        let mut read = self.memory_read_transaction().await?;
        let rows = sqlx::query(
            "SELECT m.* FROM memory_entries m
             WHERE ((m.visibility='shared' AND (
                 m.scope_type='organization'
                 OR (m.scope_type='project' AND m.project_id=?)
                 OR (m.scope_type='task' AND m.task_id=?)
             )) OR (m.scope_type='agent' AND m.agent_instance_id=?))
             AND (? OR NOT EXISTS (SELECT 1 FROM memory_entries newer
                 WHERE newer.supersedes_memory_id=m.id
                   AND (newer.visibility='shared'
                        OR (newer.scope_type='agent' AND newer.agent_instance_id=?))))
             ORDER BY m.created_at DESC,m.id DESC LIMIT ? OFFSET ?",
        )
        .bind(project_id.map(|id| id.to_string()))
        .bind(task_id.map(|id| id.to_string()))
        .bind(agent_id.map(|id| id.to_string()))
        .bind(include_superseded)
        .bind(agent_id.map(|id| id.to_string()))
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&mut *read)
        .await
        .map_err(storage)?;
        let items = rows
            .into_iter()
            .map(row_to_memory_entry)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(morrows_core::Page::from_extra_row(items, limit, offset))
    }

    pub async fn create_memory_entry(
        &self,
        input: CreateMemoryEntry,
    ) -> Result<MemoryEntry, DomainError> {
        validate_memory_input(&input)?;

        match input.scope_type.as_str() {
            "organization" => {}
            "project" => {
                self.get_project(input.project_id.expect("validated project scope"))
                    .await?;
            }
            "agent" => {
                self.get_agent(input.agent_instance_id.expect("validated agent scope"))
                    .await?;
            }
            "task" => {
                self.get_task(input.task_id.expect("validated task scope"))
                    .await?;
            }
            _ => unreachable!("validated memory scope"),
        }

        if let Some(previous_id) = input.supersedes_memory_id {
            let previous = self.get_memory_entry(previous_id).await?;
            if previous.scope_type != input.scope_type
                || previous.project_id != input.project_id
                || previous.agent_instance_id != input.agent_instance_id
                || previous.task_id != input.task_id
            {
                return Err(DomainError::Conflict(
                    "superseded memory must have the same scope".into(),
                ));
            }
        }

        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        if input.scope_type == "project"
            && input.visibility == "shared"
            && Self::git_backend(&mut tx, &input.project_id.unwrap().to_string()).await?
        {
            return Err(DomainError::Conflict(
                "project uses Git; publish through project_memory_publish with base_commit".into(),
            ));
        }
        let id = insert_memory_entry_conn(&mut tx, &input, None).await?;
        tx.commit().await.map_err(storage)?;
        self.get_memory_entry(id).await
    }

    pub async fn get_memory_entry(&self, id: Id) -> Result<MemoryEntry, DomainError> {
        let mut read = self.memory_read_transaction().await?;
        let row = sqlx::query("SELECT * FROM memory_entries WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&mut *read)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("memory entry {id}")))?;
        row_to_memory_entry(row)
    }

    pub async fn list_memory_entries(
        &self,
        scope_type: Option<&str>,
        project_id: Option<Id>,
        agent_instance_id: Option<Id>,
        task_id: Option<Id>,
    ) -> Result<Vec<MemoryEntry>, DomainError> {
        let mut read = self.memory_read_transaction().await?;
        let project_id = project_id.map(|value| value.to_string());
        let agent_instance_id = agent_instance_id.map(|value| value.to_string());
        let task_id = task_id.map(|value| value.to_string());
        let rows = sqlx::query(
            "SELECT * FROM memory_entries
             WHERE (? IS NULL OR scope_type=?)
               AND (? IS NULL OR project_id=?)
               AND (? IS NULL OR agent_instance_id=?)
               AND (? IS NULL OR task_id=?)
             ORDER BY created_at DESC,id DESC",
        )
        .bind(scope_type)
        .bind(scope_type)
        .bind(project_id.as_deref())
        .bind(project_id.as_deref())
        .bind(agent_instance_id.as_deref())
        .bind(agent_instance_id.as_deref())
        .bind(task_id.as_deref())
        .bind(task_id.as_deref())
        .fetch_all(&mut *read)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_memory_entry).collect()
    }

    pub async fn memories_for_task(
        &self,
        task_id: Id,
        agent_id: Id,
    ) -> Result<Vec<MemoryEntry>, DomainError> {
        let task = self.get_task(task_id).await?;
        let mut read = self.memory_read_transaction().await?;
        let project_id = task.project_id.map(|value| value.to_string());
        let rows = sqlx::query(
            "SELECT * FROM memory_entries
             WHERE (
               visibility='shared'
               AND (
                 scope_type='organization'
                 OR (scope_type='project' AND project_id=?)
                 OR (scope_type='task' AND task_id=?)
                 OR (scope_type='agent' AND agent_instance_id=?)
               )
             )
             OR (
               scope_type='agent' AND agent_instance_id=?
             )
             ORDER BY
               CASE scope_type
                 WHEN 'organization' THEN 0
                 WHEN 'project' THEN 1
                 WHEN 'agent' THEN 2
                 WHEN 'task' THEN 3
                 ELSE 4
               END,
               created_at,id",
        )
        .bind(project_id.as_deref())
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .bind(agent_id.to_string())
        .fetch_all(&mut *read)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_memory_entry).collect()
    }
}

pub(super) async fn task_writer_conn(
    conn: &mut sqlx::SqliteConnection,
    task_id: Id,
    agent_id: Id,
) -> Result<Task, DomainError> {
    let task = row_to_task(
        sqlx::query("SELECT * FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_optional(&mut *conn)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?,
    )?;
    let allowed: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agent_instances WHERE id=? AND (? OR EXISTS(SELECT 1 FROM assignments WHERE task_id=? AND agent_instance_id=?) OR EXISTS(SELECT 1 FROM sessions WHERE task_id=? AND agent_instance_id=? AND status='open')))")
        .bind(agent_id.to_string()).bind(task.owner_actor_id == format!("agent:{agent_id}"))
        .bind(task_id.to_string()).bind(agent_id.to_string()).bind(task_id.to_string()).bind(agent_id.to_string())
        .fetch_one(&mut *conn).await.map_err(storage)?;
    if !allowed {
        return Err(DomainError::Conflict("task is not owned by, assigned to, or shared through an open Task Session with the authenticated agent instance".into()));
    }
    Ok(task)
}

fn validate_memory_input(input: &CreateMemoryEntry) -> Result<(), DomainError> {
    let title = input.title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(DomainError::InvalidInput(
            "memory title must contain 1 to 200 characters".into(),
        ));
    }
    if input.source_kind.trim().is_empty() {
        return Err(DomainError::InvalidInput(
            "memory source_kind cannot be empty".into(),
        ));
    }
    if !matches!(input.visibility.as_str(), "shared" | "private") {
        return Err(DomainError::InvalidInput(
            "memory visibility must be shared or private".into(),
        ));
    }

    let valid_scope = match input.scope_type.as_str() {
        "organization" => {
            input.project_id.is_none()
                && input.agent_instance_id.is_none()
                && input.task_id.is_none()
        }
        "project" => {
            input.project_id.is_some()
                && input.agent_instance_id.is_none()
                && input.task_id.is_none()
        }
        "agent" => {
            input.project_id.is_none()
                && input.agent_instance_id.is_some()
                && input.task_id.is_none()
        }
        "task" => {
            input.project_id.is_none()
                && input.agent_instance_id.is_none()
                && input.task_id.is_some()
        }
        _ => false,
    };
    if !valid_scope {
        return Err(DomainError::InvalidInput(
            "memory scope does not match its scope identifier".into(),
        ));
    }
    Ok(())
}

fn row_to_memory_entry(row: sqlx::sqlite::SqliteRow) -> Result<MemoryEntry, DomainError> {
    let content: String = row.try_get("content_json").map_err(storage)?;
    Ok(MemoryEntry {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        scope_type: row.try_get("scope_type").map_err(storage)?,
        project_id: parse_opt_id(row.try_get("project_id").map_err(storage)?)?,
        agent_instance_id: parse_opt_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        task_id: parse_opt_id(row.try_get("task_id").map_err(storage)?)?,
        title: row.try_get("title").map_err(storage)?,
        content: serde_json::from_str(&content).map_err(|e| DomainError::Storage(e.to_string()))?,
        source_kind: row.try_get("source_kind").map_err(storage)?,
        source_ref: row.try_get("source_ref").map_err(storage)?,
        visibility: row.try_get("visibility").map_err(storage)?,
        supersedes_memory_id: parse_opt_id(row.try_get("supersedes_memory_id").map_err(storage)?)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
        provenance: parse_opt_json(row.try_get("provenance_json").map_err(storage)?)?,
    })
}

async fn insert_memory_entry_conn(
    conn: &mut sqlx::SqliteConnection,
    input: &CreateMemoryEntry,
    requested_id: Option<Id>,
) -> Result<Id, DomainError> {
    let id = requested_id.unwrap_or_else(Uuid::new_v4);
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO memory_entries(
                id,scope_type,project_id,agent_instance_id,task_id,title,content_json,
                source_kind,source_ref,visibility,supersedes_memory_id,created_at,updated_at
             ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(&input.scope_type)
    .bind(input.project_id.map(|value| value.to_string()))
    .bind(input.agent_instance_id.map(|value| value.to_string()))
    .bind(input.task_id.map(|value| value.to_string()))
    .bind(input.title.trim())
    .bind(serde_json::to_string(&input.content).map_err(|e| DomainError::Storage(e.to_string()))?)
    .bind(input.source_kind.trim())
    .bind(input.source_ref.as_deref())
    .bind(&input.visibility)
    .bind(input.supersedes_memory_id.map(|value| value.to_string()))
    .bind(now.to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(&mut *conn)
    .await
    .map_err(storage)?;

    Ok(id)
}
