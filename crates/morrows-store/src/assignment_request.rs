use super::*;
use morrows_core::{AssignmentRequest, Page};

impl Store {
    pub async fn request_assignment(
        &self,
        task_id: Id,
        agent_id: Id,
        role: &str,
        reason: &str,
    ) -> Result<AssignmentRequest, DomainError> {
        self.get_agent(agent_id).await?;
        let role = role.trim();
        let reason = reason.trim();
        if role.is_empty() || role.len() > 100 || reason.is_empty() {
            return Err(DomainError::InvalidInput(
                "provide role (1..100 bytes) and reason".into(),
            ));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let task = row_to_task(
            sqlx::query("SELECT * FROM tasks WHERE id=?")
                .bind(task_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?,
        )?;
        if matches!(task.state, TaskState::Done | TaskState::Cancelled) {
            return Err(DomainError::Conflict(format!("task is {}", task.state)));
        }
        if let Some(row) = sqlx::query("SELECT * FROM assignment_requests WHERE task_id=? AND agent_instance_id=? AND role=? AND status='pending'")
            .bind(task_id.to_string()).bind(agent_id.to_string()).bind(role).fetch_optional(&mut *tx).await.map_err(storage)? {
            let previous = row_to_request(row)?;
            if previous.reason != reason { return Err(DomainError::Conflict(format!("pending request {} already exists with a different reason; withdraw it before submitting a replacement",previous.id))); }
            return Ok(previous);
        }
        let assigned: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assignments WHERE task_id=? AND agent_instance_id=? AND role=? AND status='active' AND expires_at>?)")
            .bind(task_id.to_string()).bind(agent_id.to_string()).bind(role).bind(Utc::now().to_rfc3339()).fetch_one(&mut *tx).await.map_err(storage)?;
        if assigned {
            return Err(DomainError::Conflict(
                "already assigned; read task_context for execution state".into(),
            ));
        }
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO assignment_requests(id,task_id,agent_instance_id,role,reason,status,created_at) VALUES(?,?,?,?,?,'pending',?)")
            .bind(id.to_string()).bind(task_id.to_string()).bind(agent_id.to_string()).bind(role).bind(reason).bind(Utc::now().to_rfc3339())
            .execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task",
            task_id,
            "assignment.requested",
            json!({"request_id":id,"role":role}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_assignment_request(id).await
    }

    pub async fn get_assignment_request(&self, id: Id) -> Result<AssignmentRequest, DomainError> {
        row_to_request(
            sqlx::query("SELECT * FROM assignment_requests WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("assignment request {id}")))?,
        )
    }

    pub async fn assignment_requests_page(
        &self,
        task_id: Option<Id>,
        agent_id: Option<Id>,
        include_resolved: bool,
        limit: i64,
        offset: i64,
    ) -> Result<Page<AssignmentRequest>, DomainError> {
        super::discovery::validate_page(limit, offset)?;
        let task = task_id.map(|id| id.to_string());
        let agent = agent_id.map(|id| id.to_string());
        let rows = sqlx::query("SELECT * FROM assignment_requests WHERE (? IS NULL OR task_id=?) AND (? IS NULL OR agent_instance_id=?) AND (? OR status='pending') ORDER BY created_at DESC,id DESC LIMIT ? OFFSET ?")
            .bind(&task).bind(&task).bind(&agent).bind(&agent).bind(include_resolved).bind(limit+1).bind(offset)
            .fetch_all(&self.pool).await.map_err(storage)?;
        Ok(Page::from_extra_row(
            rows.into_iter()
                .map(row_to_request)
                .collect::<Result<Vec<_>, _>>()?,
            limit,
            offset,
        ))
    }

    /// Only the control-plane route passes no employee actor. Approval invokes
    /// the normal atomic claim path, never releases another worker's assignment,
    /// and does not launch work. Employee callers can only withdraw their own.
    pub async fn resolve_assignment_request(
        &self,
        id: Id,
        employee: Option<Id>,
        action: &str,
        resolution: &str,
        lease_seconds: i64,
    ) -> Result<AssignmentRequest, DomainError> {
        let status = match (employee, action) {
            (None, "approve") => "approved",
            (None, "reject") => "rejected",
            (Some(_), "withdraw") => "withdrawn",
            _ => {
                return Err(DomainError::InvalidInput(
                    "control plane may approve/reject; employee may withdraw".into(),
                ));
            }
        };
        if resolution.trim().is_empty() {
            return Err(DomainError::InvalidInput(
                "resolution reason is required".into(),
            ));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let request = row_to_request(
            sqlx::query("SELECT * FROM assignment_requests WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("assignment request {id}")))?,
        )?;
        if employee.is_some_and(|agent| agent != request.agent_instance_id) {
            return Err(DomainError::Conflict(
                "assignment request belongs to another agent".into(),
            ));
        }
        if request.status == status {
            return Ok(request);
        }
        if request.status != "pending" {
            return Err(DomainError::Conflict(format!(
                "request is {}",
                request.status
            )));
        }
        let assignment = if status == "approved" {
            let state: String = sqlx::query_scalar("SELECT state FROM tasks WHERE id=?")
                .bind(request.task_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?;
            if matches!(state.as_str(), "done" | "cancelled") {
                return Err(DomainError::Conflict(format!("task is {state}")));
            }
            // A scheduler may already have assigned this requester while it waited.
            let existing = sqlx::query("SELECT * FROM assignments WHERE task_id=? AND agent_instance_id=? AND role=? AND status='active' AND expires_at>?")
                .bind(request.task_id.to_string()).bind(request.agent_instance_id.to_string()).bind(&request.role).bind(Utc::now().to_rfc3339())
                .fetch_optional(&mut *tx).await.map_err(storage)?;
            Some(match existing {
                Some(row) => row_to_assignment(row)?.id,
                None => {
                    claim_task_tx(
                        &mut tx,
                        request.task_id,
                        request.agent_instance_id,
                        &request.role,
                        lease_seconds,
                    )
                    .await?
                    .id
                }
            })
        } else {
            None
        };
        sqlx::query("UPDATE assignment_requests SET status=?,assignment_id=?,resolution=?,resolved_at=? WHERE id=?")
            .bind(status).bind(assignment.map(|id|id.to_string())).bind(resolution.trim()).bind(Utc::now().to_rfc3339()).bind(id.to_string())
            .execute(&mut *tx).await.map_err(storage)?;
        let actor = employee.map_or_else(|| "control-plane".into(), |id| id.to_string());
        append_event_tx(&mut tx,if employee.is_some(){"agent_instance"}else{"operator"},&actor,"task",request.task_id,"assignment.request_resolved",
            json!({"request_id":id,"status":status,"assignment_id":assignment,"resolution":resolution}),None).await?;
        tx.commit().await.map_err(storage)?;
        self.get_assignment_request(id).await
    }
}

fn row_to_request(row: sqlx::sqlite::SqliteRow) -> Result<AssignmentRequest, DomainError> {
    Ok(AssignmentRequest {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        task_id: parse_id(row.try_get("task_id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        role: row.try_get("role").map_err(storage)?,
        reason: row.try_get("reason").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        assignment_id: parse_opt_id(row.try_get("assignment_id").map_err(storage)?)?,
        resolution: row.try_get("resolution").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        resolved_at: parse_opt_dt(row.try_get("resolved_at").map_err(storage)?)?,
    })
}
