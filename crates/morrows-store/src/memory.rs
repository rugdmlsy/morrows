use super::*;
use morrows_core::{CreateMemoryEntry, MemoryEntry};

impl Store {
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
        .fetch_all(&self.pool)
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

        let id = Uuid::new_v4();
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
        .bind(
            serde_json::to_string(&input.content)
                .map_err(|e| DomainError::Storage(e.to_string()))?,
        )
        .bind(input.source_kind.trim())
        .bind(input.source_ref.as_deref())
        .bind(&input.visibility)
        .bind(input.supersedes_memory_id.map(|value| value.to_string()))
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        self.get_memory_entry(id).await
    }

    pub async fn get_memory_entry(&self, id: Id) -> Result<MemoryEntry, DomainError> {
        let row = sqlx::query("SELECT * FROM memory_entries WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
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
        .fetch_all(&self.pool)
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
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_memory_entry).collect()
    }
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
    })
}
