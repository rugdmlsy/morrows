use super::*;
use morrows_core::{
    AssignmentIntake, INTAKE_PHASE_CONTEXT_REVIEW, INTAKE_PHASE_HUMAN_INTERVIEW,
    INTAKE_PHASE_IMPLEMENTING, INTAKE_PHASE_READY, InterviewSubmission, TaskIntakeView,
};

impl Store {
    pub async fn get_assignment_intake(
        &self,
        assignment_id: Id,
    ) -> Result<AssignmentIntake, DomainError> {
        let row = sqlx::query("SELECT * FROM assignment_intakes WHERE assignment_id=?")
            .bind(assignment_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("assignment intake {assignment_id}")))?;
        row_to_assignment_intake(row)
    }

    pub async fn task_intake_page(
        &self,
        task_id: Id,
        agent_id: Id,
        limit: i64,
        offset: i64,
    ) -> Result<TaskIntakeView, DomainError> {
        super::discovery::validate_page(limit, offset)?;
        let assignment = self.active_executor_assignment(task_id, agent_id).await?;
        if assignment.phase == INTAKE_PHASE_IMPLEMENTING {
            return Err(DomainError::Conflict(
                "task execution already started; intake receipts are no longer mutable".into(),
            ));
        }
        let task = self.get_task(task_id).await?;
        let project_id = task.project_id.ok_or_else(|| {
            DomainError::Conflict(
                "task intake blocked: task has no project; attach it to a project before context review"
                    .into(),
            )
        })?;
        let project = self.get_project(project_id).await?;
        let project_memory = self
            .context_memories_page(None, Some(project_id), None, false, limit, offset)
            .await?;
        let head_after_read = self.project_memory_head(project_id).await?;
        if head_after_read != project.memory_head {
            return Err(DomainError::Conflict(
                "project memory changed while reading intake; retry from offset 0".into(),
            ));
        }

        let context_package = match self.get_latest_context_package(task_id).await? {
            Some(package) if package.context_snapshot_id == task.current_context_revision_id => {
                package
            }
            _ => {
                self.assemble_context_package(task_id, None, Some(agent_id))
                    .await?
            }
        };
        self.record_intake_read(
            &assignment,
            project_id,
            project.memory_head.clone(),
            offset,
            project_memory.next_offset,
            &context_package,
        )
        .await?;
        let assignment = self.get_assignment(assignment.id).await?;
        let intake = self.get_assignment_intake(assignment.id).await?;
        let blockers = self.intake_blockers(&assignment, &intake).await?;
        Ok(TaskIntakeView {
            assignment,
            intake,
            project,
            project_memory,
            context_package,
            execution_ready: blockers.is_empty(),
            blockers,
        })
    }

    pub async fn submit_intake_interview(
        &self,
        task_id: Id,
        agent_id: Id,
        input: InterviewSubmission,
    ) -> Result<AssignmentIntake, DomainError> {
        if input.understanding.trim().is_empty() {
            return Err(DomainError::InvalidInput(
                "interview understanding is required".into(),
            ));
        }
        if value_is_empty(&input.plan) {
            return Err(DomainError::InvalidInput(
                "interview plan is required".into(),
            ));
        }
        if input.questions.iter().any(|q| q.trim().is_empty()) {
            return Err(DomainError::InvalidInput(
                "interview questions cannot contain empty items".into(),
            ));
        }
        let assignment = self.active_executor_assignment(task_id, agent_id).await?;
        if !matches!(
            assignment.phase.as_str(),
            INTAKE_PHASE_CONTEXT_REVIEW | INTAKE_PHASE_HUMAN_INTERVIEW
        ) {
            return Err(DomainError::Conflict(format!(
                "assignment intake phase is {}; interview may only be submitted during context_review or human_interview",
                assignment.phase
            )));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let intake = load_intake_tx(&mut tx, assignment.id).await?;
        let blockers = intake_read_blockers_tx(self, &mut tx, &assignment, &intake).await?;
        if !blockers.is_empty() {
            return Err(DomainError::Conflict(format!(
                "task intake context review is incomplete: {}",
                blockers.join(", ")
            )));
        }
        let now = Utc::now();
        let questions: Vec<String> = input
            .questions
            .into_iter()
            .map(|q| q.trim().to_owned())
            .collect();
        sqlx::query(
            "UPDATE assignment_intakes SET understanding=?,constraints_json=?,plan_json=?,questions_json=?,unresolved_questions_json=?,interview_status='pending',human_response=NULL,approved_by_actor_id=NULL,approved_at=NULL,updated_at=? WHERE assignment_id=?",
        )
        .bind(input.understanding.trim())
        .bind(input.constraints.to_string())
        .bind(input.plan.to_string())
        .bind(json!(questions).to_string())
        .bind(json!(questions).to_string())
        .bind(now.to_rfc3339())
        .bind(assignment.id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query("UPDATE assignments SET phase=? WHERE id=? AND status='active'")
            .bind(INTAKE_PHASE_HUMAN_INTERVIEW)
            .bind(assignment.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task",
            task_id,
            "intake.interview_submitted",
            json!({"assignment_id":assignment.id,"question_count":questions.len()}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_assignment_intake(assignment.id).await
    }

    pub async fn resolve_intake_interview(
        &self,
        assignment_id: Id,
        action: &str,
        response: &str,
        actor_id: &str,
    ) -> Result<AssignmentIntake, DomainError> {
        if !matches!(action, "approve" | "revise") {
            return Err(DomainError::InvalidInput(
                "interview action must be approve or revise".into(),
            ));
        }
        if action == "revise" && response.trim().is_empty() {
            return Err(DomainError::InvalidInput(
                "revision response is required".into(),
            ));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let assignment = row_to_assignment(
            sqlx::query("SELECT * FROM assignments WHERE id=?")
                .bind(assignment_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("assignment {assignment_id}")))?,
        )?;
        if assignment.role != "executor" || assignment.status != "active" {
            return Err(DomainError::Conflict(
                "human interview requires an active executor assignment".into(),
            ));
        }
        if assignment.phase != INTAKE_PHASE_HUMAN_INTERVIEW {
            return Err(DomainError::Conflict(format!(
                "assignment intake phase is {}; expected human_interview",
                assignment.phase
            )));
        }
        let intake = load_intake_tx(&mut tx, assignment.id).await?;
        if intake.interview_status != "pending" {
            return Err(DomainError::Conflict(format!(
                "interview status is {}; the agent must submit or resubmit the interview before human resolution",
                intake.interview_status
            )));
        }
        if action == "approve"
            && !intake.unresolved_questions.is_empty()
            && response.trim().is_empty()
        {
            return Err(DomainError::InvalidInput(
                "approval response is required when the interview contains unresolved questions"
                    .into(),
            ));
        }
        let read_blockers = intake_read_blockers_tx(self, &mut tx, &assignment, &intake).await?;
        if !read_blockers.is_empty() {
            return Err(DomainError::Conflict(format!(
                "task intake became stale before human resolution: {}",
                read_blockers.join(", ")
            )));
        }
        let now = Utc::now();
        if action == "approve" {
            sqlx::query(
                "UPDATE assignment_intakes SET unresolved_questions_json='[]',interview_status='approved',human_response=?,approved_by_actor_id=?,approved_at=?,updated_at=? WHERE assignment_id=?",
            )
            .bind(response.trim())
            .bind(actor_id)
            .bind(now.to_rfc3339())
            .bind(now.to_rfc3339())
            .bind(assignment.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query("UPDATE assignments SET phase=? WHERE id=? AND status='active'")
                .bind(INTAKE_PHASE_READY)
                .bind(assignment.id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
        } else {
            sqlx::query(
                "UPDATE assignment_intakes SET interview_status='revision_requested',human_response=?,approved_by_actor_id=NULL,approved_at=NULL,updated_at=? WHERE assignment_id=?",
            )
            .bind(response.trim())
            .bind(now.to_rfc3339())
            .bind(assignment.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }
        append_event_tx(
            &mut tx,
            "operator",
            actor_id,
            "task",
            assignment.task_id,
            if action == "approve" {
                "intake.interview_approved"
            } else {
                "intake.interview_revision_requested"
            },
            json!({"assignment_id":assignment.id,"response":response.trim()}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_assignment_intake(assignment.id).await
    }

    pub async fn begin_task_execution(
        &self,
        task_id: Id,
        agent_id: Id,
    ) -> Result<Assignment, DomainError> {
        let assignment = self.active_executor_assignment(task_id, agent_id).await?;
        if assignment.phase == INTAKE_PHASE_IMPLEMENTING {
            return Ok(assignment);
        }
        if assignment.phase != INTAKE_PHASE_READY {
            return Err(DomainError::Conflict(format!(
                "task execution blocked: assignment intake phase is {}",
                assignment.phase
            )));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let intake = load_intake_tx(&mut tx, assignment.id).await?;
        let blockers = intake_execution_blockers_tx(self, &mut tx, &assignment, &intake).await?;
        if !blockers.is_empty() {
            return Err(DomainError::Conflict(format!(
                "task execution blocked by intake: {}",
                blockers.join(", ")
            )));
        }
        let changed = sqlx::query(
            "UPDATE assignments SET phase=? WHERE id=? AND status='active' AND phase=?",
        )
        .bind(INTAKE_PHASE_IMPLEMENTING)
        .bind(assignment.id.to_string())
        .bind(INTAKE_PHASE_READY)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        if changed.rows_affected() != 1 {
            return Err(DomainError::Conflict(
                "assignment intake phase changed concurrently".into(),
            ));
        }
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task",
            task_id,
            "intake.execution_started",
            json!({"assignment_id":assignment.id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_assignment(assignment.id).await
    }

    pub async fn intake_blockers(
        &self,
        assignment: &Assignment,
        intake: &AssignmentIntake,
    ) -> Result<Vec<String>, DomainError> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let blockers = intake_execution_blockers_tx(self, &mut tx, assignment, intake).await?;
        tx.rollback().await.map_err(storage)?;
        Ok(blockers)
    }

    async fn active_executor_assignment(
        &self,
        task_id: Id,
        agent_id: Id,
    ) -> Result<Assignment, DomainError> {
        let row = sqlx::query(
            "SELECT * FROM assignments WHERE task_id=? AND agent_instance_id=? AND role='executor' AND status='active' AND expires_at>? ORDER BY acquired_at DESC LIMIT 1",
        )
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .bind(Utc::now().to_rfc3339())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::Conflict("no active executor assignment for caller".into()))?;
        row_to_assignment(row)
    }

    async fn record_intake_read(
        &self,
        assignment: &Assignment,
        project_id: Id,
        memory_head: Option<String>,
        offset: i64,
        next_offset: Option<i64>,
        package: &morrows_core::ContextPackage,
    ) -> Result<(), DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let intake = load_intake_tx(&mut tx, assignment.id).await?;
        let same_snapshot = intake.project_id == Some(project_id)
            && intake.project_memory_head == memory_head
            && (intake.project_memory_read_at.is_some()
                || intake.project_memory_next_offset.is_some()
                || intake.project_memory_complete);
        if !same_snapshot && offset != 0 {
            return Err(DomainError::Conflict(
                "project memory snapshot changed; restart task_intake from offset 0".into(),
            ));
        }
        if same_snapshot && !intake.project_memory_complete {
            let expected = intake.project_memory_next_offset.unwrap_or(0);
            if offset != expected {
                return Err(DomainError::Conflict(format!(
                    "task_intake expected project memory offset {expected}; received {offset}"
                )));
            }
        }
        let snapshot_changed = intake.project_id != Some(project_id)
            || intake.project_memory_head != memory_head
            || intake.context_package_id != Some(package.id)
            || intake.context_revision_id != package.context_snapshot_id;
        if snapshot_changed && intake.interview_status != "not_started" {
            sqlx::query("UPDATE assignments SET phase=? WHERE id=? AND status='active'")
                .bind(INTAKE_PHASE_CONTEXT_REVIEW)
                .bind(assignment.id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            sqlx::query(
                "UPDATE assignment_intakes SET understanding='',constraints_json='{}',plan_json='[]',questions_json='[]',unresolved_questions_json='[]',interview_status='not_started',human_response=NULL,approved_by_actor_id=NULL,approved_at=NULL WHERE assignment_id=?",
            )
            .bind(assignment.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }
        let now = Utc::now();
        let complete = next_offset.is_none();
        sqlx::query(
            "UPDATE assignment_intakes SET project_id=?,project_memory_head=?,project_memory_next_offset=?,project_memory_complete=?,project_memory_read_at=?,context_package_id=?,context_revision_id=?,context_package_read_at=?,updated_at=? WHERE assignment_id=?",
        )
        .bind(project_id.to_string())
        .bind(&memory_head)
        .bind(next_offset)
        .bind(complete)
        .bind(complete.then(|| now.to_rfc3339()))
        .bind(package.id.to_string())
        .bind(package.context_snapshot_id.map(|id| id.to_string()))
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(assignment.id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &assignment.agent_instance_id.to_string(),
            "task",
            assignment.task_id,
            "intake.context_read",
            json!({
                "assignment_id":assignment.id,
                "project_id":project_id,
                "project_memory_head":memory_head,
                "offset":offset,
                "next_offset":next_offset,
                "project_memory_complete":complete,
                "context_package_id":package.id,
                "context_revision_id":package.context_snapshot_id,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(())
    }
}

pub(super) async fn create_assignment_intake_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    assignment_id: Id,
    task_id: Id,
    agent_id: Id,
) -> Result<(), DomainError> {
    let project_id: Option<String> = sqlx::query_scalar("SELECT project_id FROM tasks WHERE id=?")
        .bind(task_id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?
        .flatten();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO assignment_intakes(assignment_id,task_id,agent_instance_id,project_id,created_at,updated_at) VALUES(?,?,?,?,?,?)",
    )
    .bind(assignment_id.to_string())
    .bind(task_id.to_string())
    .bind(agent_id.to_string())
    .bind(project_id)
    .bind(&now)
    .bind(&now)
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

pub(super) async fn enforce_execution_phase_tx(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    assignment: &Assignment,
) -> Result<(), DomainError> {
    if assignment.role != "executor" || assignment.phase == INTAKE_PHASE_IMPLEMENTING {
        return Ok(());
    }
    if assignment.phase != INTAKE_PHASE_READY {
        return Err(DomainError::Conflict(format!(
            "task execution blocked: intake phase is {}; complete task_intake and human interview first",
            assignment.phase
        )));
    }
    let intake = load_intake_tx(tx, assignment.id).await?;
    let blockers = intake_execution_blockers_tx(store, tx, assignment, &intake).await?;
    if !blockers.is_empty() {
        return Err(DomainError::Conflict(format!(
            "task execution blocked by intake: {}",
            blockers.join(", ")
        )));
    }
    sqlx::query("UPDATE assignments SET phase=? WHERE id=? AND status='active' AND phase=?")
        .bind(INTAKE_PHASE_IMPLEMENTING)
        .bind(assignment.id.to_string())
        .bind(INTAKE_PHASE_READY)
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
    Ok(())
}

async fn load_intake_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    assignment_id: Id,
) -> Result<AssignmentIntake, DomainError> {
    row_to_assignment_intake(
        sqlx::query("SELECT * FROM assignment_intakes WHERE assignment_id=?")
            .bind(assignment_id.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("assignment intake {assignment_id}")))?,
    )
}

async fn intake_read_blockers_tx(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    assignment: &Assignment,
    intake: &AssignmentIntake,
) -> Result<Vec<String>, DomainError> {
    let mut blockers = Vec::new();
    let row = sqlx::query("SELECT project_id,current_context_revision_id FROM tasks WHERE id=?")
        .bind(assignment.task_id.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage)?;
    let current_project = parse_opt_id(row.try_get("project_id").map_err(storage)?)?;
    let current_context = parse_opt_id(
        row.try_get("current_context_revision_id")
            .map_err(storage)?,
    )?;
    let Some(project_id) = current_project else {
        blockers.push("missing_project".into());
        return Ok(blockers);
    };
    if intake.project_id != Some(project_id) {
        blockers.push("project_changed_or_unread".into());
    }
    let memory_backend: Option<String> =
        sqlx::query_scalar("SELECT backend FROM memory_git_domains WHERE domain=?")
            .bind(project_id.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage)?;
    let current_head = if memory_backend.as_deref() == Some("git") {
        store
            .project_memory_head_conn(&mut **tx, project_id)
            .await?
    } else {
        None
    };
    if !intake.project_memory_complete || intake.project_memory_read_at.is_none() {
        blockers.push("project_memory_unread".into());
    } else if intake.project_memory_head != current_head {
        blockers.push("project_memory_stale".into());
    }
    let latest_package = sqlx::query(
        "SELECT id,context_snapshot_id FROM context_packages WHERE work_item_id=? ORDER BY created_at DESC LIMIT 1",
    )
    .bind(assignment.task_id.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    match latest_package {
        None => blockers.push("context_package_missing".into()),
        Some(row) => {
            let package_id = parse_id(row.try_get("id").map_err(storage)?)?;
            let package_context =
                parse_opt_id(row.try_get("context_snapshot_id").map_err(storage)?)?;
            if intake.context_package_id != Some(package_id)
                || intake.context_package_read_at.is_none()
            {
                blockers.push("context_package_unread_or_stale".into());
            }
            if package_context != current_context || intake.context_revision_id != current_context {
                blockers.push("task_context_changed".into());
            }
        }
    }
    Ok(blockers)
}

async fn intake_execution_blockers_tx(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    assignment: &Assignment,
    intake: &AssignmentIntake,
) -> Result<Vec<String>, DomainError> {
    let mut blockers = intake_read_blockers_tx(store, tx, assignment, intake).await?;
    if intake.interview_status != "approved" || intake.approved_at.is_none() {
        blockers.push("human_interview_not_approved".into());
    }
    if !intake.unresolved_questions.is_empty() {
        blockers.push("unresolved_questions".into());
    }
    Ok(blockers)
}

fn row_to_assignment_intake(row: sqlx::sqlite::SqliteRow) -> Result<AssignmentIntake, DomainError> {
    Ok(AssignmentIntake {
        assignment_id: parse_id(row.try_get("assignment_id").map_err(storage)?)?,
        task_id: parse_id(row.try_get("task_id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        project_id: parse_opt_id(row.try_get("project_id").map_err(storage)?)?,
        project_memory_head: row.try_get("project_memory_head").map_err(storage)?,
        project_memory_next_offset: row.try_get("project_memory_next_offset").map_err(storage)?,
        project_memory_complete: row
            .try_get::<i64, _>("project_memory_complete")
            .map_err(storage)?
            != 0,
        project_memory_read_at: parse_opt_dt(
            row.try_get("project_memory_read_at").map_err(storage)?,
        )?,
        context_package_id: parse_opt_id(row.try_get("context_package_id").map_err(storage)?)?,
        context_revision_id: parse_opt_id(row.try_get("context_revision_id").map_err(storage)?)?,
        context_package_read_at: parse_opt_dt(
            row.try_get("context_package_read_at").map_err(storage)?,
        )?,
        understanding: row.try_get("understanding").map_err(storage)?,
        constraints: parse_json(row.try_get("constraints_json").map_err(storage)?)?,
        plan: parse_json(row.try_get("plan_json").map_err(storage)?)?,
        questions: serde_json::from_str(
            &row.try_get::<String, _>("questions_json")
                .map_err(storage)?,
        )
        .map_err(storage)?,
        unresolved_questions: serde_json::from_str(
            &row.try_get::<String, _>("unresolved_questions_json")
                .map_err(storage)?,
        )
        .map_err(storage)?,
        interview_status: row.try_get("interview_status").map_err(storage)?,
        human_response: row.try_get("human_response").map_err(storage)?,
        approved_by_actor_id: row.try_get("approved_by_actor_id").map_err(storage)?,
        approved_at: parse_opt_dt(row.try_get("approved_at").map_err(storage)?)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}

fn value_is_empty(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.trim().is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
        _ => false,
    }
}
