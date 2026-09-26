use super::*;
use morrows_core::{ContextPackage, CreateContextPackage};

impl Store {
    pub async fn create_context_package(
        &self,
        input: CreateContextPackage,
    ) -> Result<ContextPackage, DomainError> {
        self.get_task(input.work_item_id).await?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;

        sqlx::query(
            "INSERT INTO context_packages(
                id,work_item_id,objective,summary_json,context_snapshot_id,
                memory_refs_json,decision_refs_json,artifact_refs_json,
                changed_files_json,verified_results_json,blockers_json,
                unresolved_questions_json,next_action,source_run_id,
                source_agent_id,created_at
             ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)"
        )
        .bind(id.to_string())
        .bind(input.work_item_id.to_string())
        .bind(&input.objective)
        .bind(input.summary.map(|v| v.to_string()))
        .bind(input.context_snapshot_id.map(|v| v.to_string()))
        .bind(serde_json::to_string(&input.memory_refs).map_err(storage)?)
        .bind(serde_json::to_string(&input.decision_refs).map_err(storage)?)
        .bind(serde_json::to_string(&input.artifact_refs).map_err(storage)?)
        .bind(serde_json::to_string(&input.changed_files).map_err(storage)?)
        .bind(serde_json::to_string(&input.verified_results).map_err(storage)?)
        .bind(serde_json::to_string(&input.blockers).map_err(storage)?)
        .bind(serde_json::to_string(&input.unresolved_questions).map_err(storage)?)
        .bind(&input.next_action)
        .bind(input.source_run_id.map(|v| v.to_string()))
        .bind(input.source_agent_id.map(|v| v.to_string()))
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        append_event_tx(
            &mut tx,
            "system",
            "morrows",
            "context_package",
            id,
            "context_package.created",
            json!({"work_item_id": input.work_item_id}),
            None,
        )
        .await?;

        tx.commit().await.map_err(storage)?;
        self.get_context_package(id).await
    }

    pub async fn get_context_package(&self, id: Id) -> Result<ContextPackage, DomainError> {
        let row = sqlx::query("SELECT * FROM context_packages WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("context package {id}")))?;
        row_to_context_package(row)
    }

    pub async fn get_latest_context_package(
        &self,
        work_item_id: Id,
    ) -> Result<Option<ContextPackage>, DomainError> {
        self.get_task(work_item_id).await?;
        let row = sqlx::query(
            "SELECT * FROM context_packages
             WHERE work_item_id=?
             ORDER BY created_at DESC, id DESC
             LIMIT 1",
        )
        .bind(work_item_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        row.map(row_to_context_package).transpose()
    }

    pub async fn assemble_context_package(
        &self,
        work_item_id: Id,
        source_run_id: Option<Id>,
        source_agent_id: Option<Id>,
    ) -> Result<ContextPackage, DomainError> {
        let task = self.get_task(work_item_id).await?;
        let objective = if task.description.trim().is_empty() {
            task.title.clone()
        } else {
            format!("{}: {}", task.title, task.description)
        };

        let decisions = self.task_decisions(work_item_id).await?;
        let decision_refs: Vec<Id> = decisions.into_iter().map(|d| d.id).collect();

        let artifacts = self.task_artifacts(work_item_id).await?;
        let artifact_refs: Vec<Id> = artifacts.into_iter().map(|a| a.id).collect();

        let handoffs = self.task_handoffs(work_item_id).await?;
        let mut blockers = Vec::new();
        let mut next_action = String::new();
        // task_handoffs is chronological; continuation comes from the newest handoff.
        if let Some(latest_handoff) = handoffs.last() {
            blockers = latest_handoff.content.blockers.clone();
            if let Some(first_remaining) = latest_handoff.content.remaining.first() {
                next_action = first_remaining.clone();
            }
        }

        self.create_context_package(CreateContextPackage {
            work_item_id,
            objective,
            summary: None,
            context_snapshot_id: task.current_context_revision_id,
            memory_refs: vec![],
            decision_refs,
            artifact_refs,
            changed_files: vec![],
            verified_results: vec![],
            blockers,
            unresolved_questions: vec![],
            next_action,
            source_run_id,
            source_agent_id,
        })
        .await
    }
}

fn row_to_context_package(row: sqlx::sqlite::SqliteRow) -> Result<ContextPackage, DomainError> {
    let summary: Option<String> = row.try_get("summary_json").map_err(storage)?;
    let memory_refs: String = row.try_get("memory_refs_json").map_err(storage)?;
    let decision_refs: String = row.try_get("decision_refs_json").map_err(storage)?;
    let artifact_refs: String = row.try_get("artifact_refs_json").map_err(storage)?;
    let changed_files: String = row.try_get("changed_files_json").map_err(storage)?;
    let verified_results: String = row.try_get("verified_results_json").map_err(storage)?;
    let blockers: String = row.try_get("blockers_json").map_err(storage)?;
    let unresolved_questions: String = row
        .try_get("unresolved_questions_json")
        .map_err(storage)?;

    Ok(ContextPackage {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        work_item_id: parse_id(row.try_get("work_item_id").map_err(storage)?)?,
        objective: row.try_get("objective").map_err(storage)?,
        summary: summary.map(parse_json).transpose()?,
        context_snapshot_id: parse_opt_id(
            row.try_get("context_snapshot_id").map_err(storage)?,
        )?,
        memory_refs: serde_json::from_str(&memory_refs).unwrap_or_default(),
        decision_refs: serde_json::from_str(&decision_refs).unwrap_or_default(),
        artifact_refs: serde_json::from_str(&artifact_refs).unwrap_or_default(),
        changed_files: serde_json::from_str(&changed_files).unwrap_or_default(),
        verified_results: serde_json::from_str(&verified_results).unwrap_or_default(),
        blockers: serde_json::from_str(&blockers).unwrap_or_default(),
        unresolved_questions: serde_json::from_str(&unresolved_questions).unwrap_or_default(),
        next_action: row.try_get("next_action").map_err(storage)?,
        source_run_id: parse_opt_id(row.try_get("source_run_id").map_err(storage)?)?,
        source_agent_id: parse_opt_id(row.try_get("source_agent_id").map_err(storage)?)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}
