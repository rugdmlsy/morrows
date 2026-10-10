use super::*;
use morrows_core::TaskRelationship;

impl Store {
    pub async fn task_relationships(
        &self,
        task_id: Id,
    ) -> Result<Vec<TaskRelationship>, DomainError> {
        self.get_task(task_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM task_relationships
             WHERE source_task_id=? OR target_task_id=?
             ORDER BY created_at,source_task_id,target_task_id,relation_type",
        )
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_task_relationship).collect()
    }

    pub async fn task_relationships_page(
        &self,
        task_id: Id,
        limit: i64,
        offset: i64,
    ) -> Result<Value, DomainError> {
        super::discovery::validate_page(limit, offset)?;
        self.get_task(task_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM task_relationships
             WHERE source_task_id=? OR target_task_id=?
             ORDER BY created_at DESC,source_task_id DESC,target_task_id DESC,relation_type DESC
             LIMIT ? OFFSET ?",
        )
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(limit + 1)
        .bind(offset)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?
        .into_iter()
        .map(row_to_task_relationship)
        .collect::<Result<Vec<_>, _>>()?;
        serde_json::to_value(morrows_core::Page::from_extra_row(rows, limit, offset))
            .map_err(storage)
    }

    pub async fn create_rework_task(
        &self,
        source_task_id: Id,
        owner_actor_id: String,
        created_by_actor_id: String,
        title: Option<String>,
        description: Option<String>,
        reason: String,
    ) -> Result<(Task, TaskRelationship), DomainError> {
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(DomainError::InvalidInput(
                "rework reason cannot be empty".into(),
            ));
        }

        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let source_row = sqlx::query("SELECT * FROM tasks WHERE id=?")
            .bind(source_task_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {source_task_id}")))?;
        let source = row_to_task(source_row)?;
        if source.state != TaskState::Done {
            return Err(DomainError::Conflict(format!(
                "rework can only be created from a completed task; task is {}",
                source.state
            )));
        }

        let source_rework = sqlx::query(
            "SELECT target_task_id,metadata_json FROM task_relationships
             WHERE source_task_id=? AND relation_type='rework_of'
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(source_task_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?;
        let root_task_id = if let Some(row) = source_rework {
            let target: String = row.try_get("target_task_id").map_err(storage)?;
            let metadata: Value =
                serde_json::from_str(&row.try_get::<String, _>("metadata_json").map_err(storage)?)
                    .map_err(storage)?;
            metadata
                .get("root_task_id")
                .and_then(Value::as_str)
                .map(|value| parse_id(value.to_owned()))
                .transpose()?
                .unwrap_or(parse_id(target)?)
        } else {
            source_task_id
        };

        let relationship_rows = sqlx::query(
            "SELECT metadata_json FROM task_relationships WHERE relation_type='rework_of'",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(storage)?;
        let root = root_task_id.to_string();
        let mut max_round = 0_i64;
        for row in relationship_rows {
            let metadata: Value =
                serde_json::from_str(&row.try_get::<String, _>("metadata_json").map_err(storage)?)
                    .map_err(storage)?;
            if metadata.get("root_task_id").and_then(Value::as_str) == Some(root.as_str()) {
                max_round = max_round.max(
                    metadata
                        .get("rework_round")
                        .and_then(Value::as_i64)
                        .unwrap_or(0),
                );
            }
        }
        let rework_round = max_round + 1;

        let title = title
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("Rework: {}", source.title));
        let description = description
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| format!("Rework reason: {reason}\n\nSource task: {source_task_id}"));
        let task_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO tasks(
                id,project_id,title,description,owner_actor_id,state,assignment_mode,
                priority,created_at,updated_at,acceptance_criteria_json
             ) VALUES(?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(task_id.to_string())
        .bind(source.project_id.map(|id| id.to_string()))
        .bind(&title)
        .bind(&description)
        .bind(&owner_actor_id)
        .bind(TaskState::Ready.to_string())
        .bind(source.assignment_mode.to_string())
        .bind(source.priority)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(serde_json::to_string(&source.acceptance_criteria).map_err(storage)?)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        crate::revision::baseline_tx(&mut tx, task_id).await?;

        let inherited_dispatch_policy_count = if source.assignment_mode == AssignmentMode::Dispatch
        {
            let enabled_policy_count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM task_dispatch_policies WHERE task_id=? AND enabled=1",
            )
            .bind(source_task_id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage)?;
            if enabled_policy_count == 0 {
                return Err(DomainError::Conflict(
                    "source task is dispatch-managed but has no enabled dispatch policy to inherit"
                        .into(),
                ));
            }
            let copied = sqlx::query(
                    "INSERT INTO task_dispatch_policies(
                        task_id,role,required_capabilities_json,profile_id,account_id,machine_id,
                        heartbeat_ttl_seconds,capacity_ttl_seconds,lease_seconds,enabled,created_at,updated_at
                     )
                     SELECT ?,role,required_capabilities_json,profile_id,account_id,machine_id,
                            heartbeat_ttl_seconds,capacity_ttl_seconds,lease_seconds,enabled,?,?
                     FROM task_dispatch_policies WHERE task_id=?",
                )
                .bind(task_id.to_string())
                .bind(now.to_rfc3339())
                .bind(now.to_rfc3339())
                .bind(source_task_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            copied.rows_affected()
        } else {
            0
        };

        let metadata = json!({
            "reason": reason,
            "root_task_id": root_task_id,
            "rework_round": rework_round,
            "inherited_dispatch_policy_count": inherited_dispatch_policy_count,
        });
        sqlx::query(
            "INSERT INTO task_relationships(
                source_task_id,target_task_id,relation_type,created_by_actor_id,
                metadata_json,created_at
             ) VALUES(?,?,?,?,?,?)",
        )
        .bind(task_id.to_string())
        .bind(source_task_id.to_string())
        .bind("rework_of")
        .bind(&created_by_actor_id)
        .bind(metadata.to_string())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        let (actor_type, actor_id) = actor_parts(&created_by_actor_id);
        append_event_tx(
            &mut tx,
            actor_type,
            actor_id,
            "task",
            task_id,
            "task.created",
            json!({
                "state":"ready",
                "assignment_mode":source.assignment_mode,
                "title":title,
                "project_id":source.project_id,
                "rework_of":source_task_id,
                "rework_round":rework_round,
            }),
            None,
        )
        .await?;
        append_event_tx(
            &mut tx,
            actor_type,
            actor_id,
            "task",
            source_task_id,
            "task.rework_created",
            json!({
                "rework_task_id":task_id,
                "reason":reason,
                "root_task_id":root_task_id,
                "rework_round":rework_round,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;

        let task = self.get_task(task_id).await?;
        let relationship = self
            .task_relationships(task_id)
            .await?
            .into_iter()
            .find(|item| {
                item.source_task_id == task_id
                    && item.target_task_id == source_task_id
                    && item.relation_type == "rework_of"
            })
            .ok_or_else(|| DomainError::Storage("created rework relationship missing".into()))?;
        Ok((task, relationship))
    }

    pub async fn reopen_task(&self, task_id: Id, reason: String) -> Result<Task, DomainError> {
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(DomainError::InvalidInput(
                "reopen reason cannot be empty".into(),
            ));
        }
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row = sqlx::query("SELECT * FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?;
        let task = row_to_task(row)?;
        if task.state != TaskState::Done {
            return Err(DomainError::Conflict(format!(
                "only completed tasks can be reopened; task is {}",
                task.state
            )));
        }

        let has_rework: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM task_relationships
                WHERE target_task_id=? AND relation_type='rework_of'
             )",
        )
        .bind(task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if has_rework {
            return Err(DomainError::Conflict(
                "task already has rework history; create another rework task instead of reopening it"
                    .into(),
            ));
        }

        let consumed_by_dependent: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM task_dependencies d
                JOIN tasks t ON t.id=d.task_id
                WHERE d.depends_on_task_id=?
                  AND t.state IN ('in_progress','review','blocked','done')
             )",
        )
        .bind(task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if consumed_by_dependent {
            return Err(DomainError::Conflict(
                "task completion has already been consumed by a dependent task; create a rework task instead"
                    .into(),
            ));
        }

        sqlx::query("UPDATE tasks SET state='ready',updated_at=? WHERE id=?")
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "operator",
            "control-plane",
            "task",
            task_id,
            "task.reopened",
            json!({"previous_state":"done","state":"ready","reason":reason}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_task(task_id).await
    }
}

fn actor_parts(actor: &str) -> (&str, &str) {
    if let Some(id) = actor.strip_prefix("agent:") {
        ("agent_instance", id)
    } else if let Some(id) = actor.strip_prefix("human:") {
        ("human", id)
    } else {
        ("operator", actor)
    }
}

pub(super) fn row_to_task_relationship(
    row: sqlx::sqlite::SqliteRow,
) -> Result<TaskRelationship, DomainError> {
    Ok(TaskRelationship {
        source_task_id: parse_id(row.try_get("source_task_id").map_err(storage)?)?,
        target_task_id: parse_id(row.try_get("target_task_id").map_err(storage)?)?,
        relation_type: row.try_get("relation_type").map_err(storage)?,
        created_by_actor_id: row.try_get("created_by_actor_id").map_err(storage)?,
        metadata: serde_json::from_str(
            &row.try_get::<String, _>("metadata_json").map_err(storage)?,
        )
        .map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}
