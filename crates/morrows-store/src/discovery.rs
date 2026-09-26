use super::*;
use morrows_core::{Page, ProjectSummary, TaskQuery, TaskSummary};

pub(crate) fn validate_page(limit: i64, offset: i64) -> Result<(), DomainError> {
    if !(1..=100).contains(&limit) || !(0..=i64::MAX - 100).contains(&offset) {
        return Err(DomainError::InvalidInput(
            "limit must be 1..100 and offset must be nonnegative and below i64::MAX - 100".into(),
        ));
    }
    Ok(())
}

impl Store {
    pub async fn task_execution_page(
        &self,
        task_id: Id,
        agent_id: Id,
        limit: i64,
        offset: i64,
    ) -> Result<Value, DomainError> {
        validate_page(limit, offset)?;
        self.get_task(task_id).await?;
        let assignments = sqlx::query(
            "SELECT * FROM assignments WHERE task_id=? ORDER BY acquired_at DESC,id DESC LIMIT ? OFFSET ?",
        ).bind(task_id.to_string()).bind(limit + 1).bind(offset)
         .fetch_all(&self.pool).await.map_err(storage)?
         .into_iter().map(row_to_assignment).collect::<Result<Vec<_>,_>>()?;
        // Checkpoints and provider-thread references are returned only for the
        // caller's executions; assignment metadata identifies other contributors.
        let runs = sqlx::query(
            "SELECT r.*,c.context_revision_id FROM runs r
             LEFT JOIN run_context_revisions c ON c.run_id=r.id
             WHERE r.task_id=? AND r.agent_instance_id=?
             ORDER BY r.started_at DESC,r.id DESC LIMIT ? OFFSET ?",
        )
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?
        .into_iter()
        .map(row_to_run)
        .collect::<Result<Vec<_>, _>>()?;
        let milestones = sqlx::query(
            "SELECT m.* FROM run_milestones m
             JOIN runs r ON r.id=m.run_id
             WHERE m.task_id=? AND r.agent_instance_id=?
             ORDER BY m.created_at DESC,m.sequence DESC LIMIT ? OFFSET ?",
        )
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?
        .into_iter()
        .map(super::milestone::row_to_run_milestone)
        .collect::<Result<Vec<_>, _>>()?;
        Ok(json!({
            "assignments": Page::from_extra_row(assignments, limit, offset),
            "my_runs": Page::from_extra_row(runs, limit, offset),
            "my_milestones": Page::from_extra_row(milestones, limit, offset),
            "continuation_policy": self.task_continuation_policy(task_id).await?,
            "recovery": self.task_recovery_context(task_id, agent_id).await?,
        }))
    }

    /// Assignment filtering is a discovery preference, never an authorization
    /// boundary. Keep the latest assignment per role, including expired leases,
    /// so interrupted work can be rediscovered without exposing a former owner's
    /// task after it has been reassigned. Released handoffs are no longer owned.
    pub async fn query_tasks(
        &self,
        filter: &TaskQuery,
        limit: i64,
        offset: i64,
    ) -> Result<Page<TaskSummary>, DomainError> {
        validate_page(limit, offset)?;
        if let Some(id) = filter.agent_instance_id {
            self.get_agent(id).await?;
        }
        if let Some(id) = filter.project_id {
            self.get_project(id).await?;
        }
        let agent = filter.agent_instance_id.map(|id| id.to_string());
        let project = filter.project_id.map(|id| id.to_string());
        let state = filter.state.map(|state| state.to_string());
        let rows = sqlx::query(
            "SELECT t.id,t.project_id,p.name AS project_name,t.title,
                    substr(t.description,1,240) AS description_preview,
                    length(t.description)>240 AS description_truncated,
                    t.state,t.priority,t.updated_at
             FROM tasks t LEFT JOIN projects p ON p.id=t.project_id
             WHERE (? IS NULL OR t.project_id=?)
               AND (? IS NULL OR t.state=?)
               AND (? IS NOT NULL OR ? OR t.state NOT IN ('done','cancelled'))
               AND (? IS NULL OR EXISTS (
                 SELECT 1 FROM assignments a WHERE a.task_id=t.id
                   AND a.agent_instance_id=?
                   AND (a.status IN ('active','expired')
                        OR (a.status='completed' AND t.state IN ('done','cancelled')))
                   AND NOT EXISTS (
                     SELECT 1 FROM assignments newer WHERE newer.task_id=a.task_id
                       AND newer.role=a.role
                       AND (newer.acquired_at>a.acquired_at OR
                            (newer.acquired_at=a.acquired_at AND newer.rowid>a.rowid))
                   )
               ))
             ORDER BY t.priority DESC,t.created_at DESC,t.id DESC LIMIT ? OFFSET ?",
        )
        .bind(project.as_deref())
        .bind(project.as_deref())
        .bind(state.as_deref())
        .bind(state.as_deref())
        .bind(state.as_deref())
        .bind(filter.include_completed)
        .bind(agent.as_deref())
        .bind(agent.as_deref())
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        let items = rows
            .into_iter()
            .map(|r| {
                Ok(TaskSummary {
                    id: parse_id(r.try_get("id").map_err(storage)?)?,
                    project_id: parse_opt_id(r.try_get("project_id").map_err(storage)?)?,
                    project_name: r.try_get("project_name").map_err(storage)?,
                    title: r.try_get("title").map_err(storage)?,
                    description_preview: r.try_get("description_preview").map_err(storage)?,
                    description_truncated: r.try_get("description_truncated").map_err(storage)?,
                    state: r.try_get::<String, _>("state").map_err(storage)?.parse()?,
                    priority: r.try_get("priority").map_err(storage)?,
                    updated_at: parse_dt(r.try_get("updated_at").map_err(storage)?)?,
                })
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
        Ok(Page::from_extra_row(items, limit, offset))
    }

    pub async fn projects_page(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Page<ProjectSummary>, DomainError> {
        validate_page(limit, offset)?;
        let rows = sqlx::query(
            "SELECT id,name,substr(description,1,240) AS description_preview,
                    length(description)>240 AS description_truncated,status,created_at,updated_at
             FROM projects ORDER BY status, name COLLATE NOCASE, id LIMIT ? OFFSET ?",
        )
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        let items = rows
            .into_iter()
            .map(|r| {
                Ok(ProjectSummary {
                    id: parse_id(r.try_get("id").map_err(storage)?)?,
                    name: r.try_get("name").map_err(storage)?,
                    description_preview: r.try_get("description_preview").map_err(storage)?,
                    description_truncated: r.try_get("description_truncated").map_err(storage)?,
                    status: r.try_get("status").map_err(storage)?,
                    created_at: parse_dt(r.try_get("created_at").map_err(storage)?)?,
                    updated_at: parse_dt(r.try_get("updated_at").map_err(storage)?)?,
                })
            })
            .collect::<Result<Vec<_>, DomainError>>()?;
        Ok(Page::from_extra_row(items, limit, offset))
    }

    pub async fn context_revisions_page(
        &self,
        task_id: Id,
        limit: i64,
        offset: i64,
    ) -> Result<Page<ContextRevision>, DomainError> {
        validate_page(limit, offset)?;
        self.get_task(task_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM context_revisions WHERE task_id=? ORDER BY version DESC LIMIT ? OFFSET ?",
        )
        .bind(task_id.to_string())
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        let items = rows
            .into_iter()
            .map(row_to_context_revision)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Page::from_extra_row(items, limit, offset))
    }

    pub async fn task_events_page(
        &self,
        task_id: Id,
        event_type: Option<&str>,
        limit: i64,
        offset: i64,
    ) -> Result<Page<Event>, DomainError> {
        validate_page(limit, offset)?;
        self.get_task(task_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM events
             WHERE ((entity_type='task' AND entity_id=?) OR json_extract(payload_json,'$.task_id')=?)
               AND (? IS NULL OR event_type=?)
             ORDER BY created_at DESC,id DESC LIMIT ? OFFSET ?",
        ).bind(task_id.to_string()).bind(task_id.to_string())
         .bind(event_type).bind(event_type).bind(limit + 1).bind(offset)
         .fetch_all(&self.pool).await.map_err(storage)?;
        let items = rows
            .into_iter()
            .map(row_to_event)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Page::from_extra_row(items, limit, offset))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn expired_work_is_discoverable_until_reassigned_and_handoffs_leave_the_inbox() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = store.register_agent("first", &[]).await.unwrap();
        let b = store.register_agent("next", &[]).await.unwrap();
        let task = store
            .create_task(serde_json::from_value(json!({"title":"Recovery"})).unwrap())
            .await
            .unwrap();
        let old = store
            .claim_task(task.id, a.id, "executor", 300)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE assignments SET status='expired',expires_at='2000-01-01T00:00:00Z' WHERE id=?",
        )
        .bind(old.id.to_string())
        .execute(&store.pool)
        .await
        .unwrap();
        let mine = TaskQuery {
            agent_instance_id: Some(a.id),
            ..Default::default()
        };
        assert_eq!(
            store.query_tasks(&mine, 20, 0).await.unwrap().items.len(),
            1
        );
        let next = store
            .claim_task(task.id, b.id, "executor", 300)
            .await
            .unwrap();
        assert!(
            store
                .query_tasks(&mine, 20, 0)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        let theirs = TaskQuery {
            agent_instance_id: Some(b.id),
            ..Default::default()
        };
        assert_eq!(
            store.query_tasks(&theirs, 20, 0).await.unwrap().items.len(),
            1
        );
        sqlx::query("UPDATE assignments SET status='released' WHERE id=?")
            .bind(next.id.to_string())
            .execute(&store.pool)
            .await
            .unwrap();
        assert!(
            store
                .query_tasks(&theirs, 20, 0)
                .await
                .unwrap()
                .items
                .is_empty()
        );
        assert_eq!(
            store
                .query_tasks(&TaskQuery::default(), 20, 0)
                .await
                .unwrap()
                .items
                .len(),
            1
        );
    }
}
