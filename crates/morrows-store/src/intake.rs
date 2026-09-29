use super::*;
use morrows_core::{
    AssignmentIntake, INTAKE_PHASE_CONTEXT_REVIEW, INTAKE_PHASE_HUMAN_INTERVIEW,
    INTAKE_PHASE_IMPLEMENTING, INTAKE_PHASE_READY, INTERVIEW_STATE_CONVERGED,
    INTERVIEW_STATE_NOT_STARTED, INTERVIEW_STATE_WAITING_FOR_AGENT, InterviewFinalize,
    TaskIntakeView,
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

    pub async fn start_intake_interview(
        &self,
        task_id: Id,
        agent_id: Id,
    ) -> Result<AssignmentIntake, DomainError> {
        let assignment = self.active_executor_assignment(task_id, agent_id).await?;
        if !matches!(
            assignment.phase.as_str(),
            INTAKE_PHASE_CONTEXT_REVIEW | INTAKE_PHASE_HUMAN_INTERVIEW
        ) {
            return Err(DomainError::Conflict(format!(
                "assignment intake phase is {}; interview may only start during context_review or human_interview",
                assignment.phase
            )));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        crate::task_graph::enforce_gate_conn(&mut tx, task_id).await?;
        let intake = load_intake_tx(&mut tx, assignment.id).await?;
        let blockers = intake_read_blockers_tx(self, &mut tx, &assignment, &intake).await?;
        if !blockers.is_empty() {
            return Err(DomainError::Conflict(format!(
                "task intake context review is incomplete: {}",
                blockers.join(", ")
            )));
        }
        let now = Utc::now();
        let session_id = match intake.interview_session_id {
            Some(id) => id,
            None => {
                self.ensure_task_session_for_agent_tx(&mut tx, task_id, agent_id)
                    .await?
            }
        };
        sqlx::query(
            "UPDATE assignment_intakes
             SET interview_session_id=?,conversation_state=?,interview_started_at=COALESCE(interview_started_at,?),
                 interview_status='pending',human_response=NULL,approved_by_actor_id=NULL,approved_at=NULL,
                 final_summary_message_id=NULL,confirmation_message_id=NULL,converged_at=NULL,updated_at=?
             WHERE assignment_id=?",
        )
        .bind(session_id.to_string())
        .bind(INTERVIEW_STATE_WAITING_FOR_AGENT)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(assignment.id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        extend_intake_lease_tx(&mut tx, assignment.id, now).await?;
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
            "intake.interview_started",
            json!({"assignment_id":assignment.id,"session_id":session_id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_assignment_intake(assignment.id).await
    }

    pub async fn finalize_intake_interview(
        &self,
        task_id: Id,
        agent_id: Id,
        input: InterviewFinalize,
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
        if !input.unresolved_questions.is_empty() {
            return Err(DomainError::Conflict(
                "human interview cannot converge while unresolved_questions is non-empty".into(),
            ));
        }
        let assignment = self.active_executor_assignment(task_id, agent_id).await?;
        if assignment.phase != INTAKE_PHASE_HUMAN_INTERVIEW {
            return Err(DomainError::Conflict(format!(
                "assignment intake phase is {}; expected human_interview",
                assignment.phase
            )));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let intake = load_intake_tx(&mut tx, assignment.id).await?;
        let session_id = intake.interview_session_id.ok_or_else(|| {
            DomainError::Conflict("human interview has no bound Task Session".into())
        })?;
        if intake.conversation_state == INTERVIEW_STATE_NOT_STARTED {
            return Err(DomainError::Conflict(format!(
                "human interview has not started"
            )));
        }
        let read_blockers = intake_read_blockers_tx(self, &mut tx, &assignment, &intake).await?;
        if !read_blockers.is_empty() {
            return Err(DomainError::Conflict(format!(
                "task intake became stale during human interview: {}",
                read_blockers.join(", ")
            )));
        }
        validate_optional_interview_audit_tx(
            &mut tx,
            session_id,
            task_id,
            agent_id,
            intake.interview_started_at,
            input.final_summary_message_id,
            input.confirmation_message_id,
        )
        .await?;
        let now = Utc::now();
        let confirmation_body: Option<String> =
            if let Some(message_id) = input.confirmation_message_id {
                sqlx::query_scalar("SELECT body FROM session_messages WHERE id=? AND session_id=?")
                    .bind(message_id.to_string())
                    .bind(session_id.to_string())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(storage)?
            } else {
                None
            };
        sqlx::query(
            "UPDATE assignment_intakes
             SET understanding=?,constraints_json=?,plan_json=?,unresolved_questions_json='[]',
                 interview_status='approved',conversation_state=?,human_response=?,
                 approved_by_actor_id=?,approved_at=?,final_summary_message_id=?,
                 confirmation_message_id=?,converged_at=?,updated_at=?
             WHERE assignment_id=?",
        )
        .bind(input.understanding.trim())
        .bind(input.constraints.to_string())
        .bind(input.plan.to_string())
        .bind(INTERVIEW_STATE_CONVERGED)
        .bind(confirmation_body)
        .bind(format!("agent_attested:{agent_id}"))
        .bind(now.to_rfc3339())
        .bind(input.final_summary_message_id.map(|id| id.to_string()))
        .bind(input.confirmation_message_id.map(|id| id.to_string()))
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(assignment.id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        if let Some(message_id) = input.confirmation_message_id {
            sqlx::query(
                "UPDATE session_messages SET status='delivered'
                 WHERE id=? AND session_id=? AND author_type='human' AND recalled_at IS NULL",
            )
            .bind(message_id.to_string())
            .bind(session_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }
        sqlx::query("UPDATE assignments SET phase=? WHERE id=? AND status='active'")
            .bind(INTAKE_PHASE_READY)
            .bind(assignment.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        extend_intake_lease_tx(&mut tx, assignment.id, now).await?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task",
            task_id,
            "intake.interview_converged",
            json!({
                "assignment_id":assignment.id,
                "session_id":session_id,
                "final_summary_message_id":input.final_summary_message_id,
                "confirmation_message_id":input.confirmation_message_id,
                "convergence_mode":"agent_attested",
            }),
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
            let mut tx = self
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(storage)?;
            crate::task_graph::enforce_gate_conn(&mut tx, task_id).await?;
            tx.commit().await.map_err(storage)?;
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
        crate::task_graph::enforce_gate_conn(&mut tx, task_id).await?;
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
        if snapshot_changed
            && (intake.interview_status != "not_started"
                || intake.conversation_state != INTERVIEW_STATE_NOT_STARTED)
        {
            sqlx::query("UPDATE assignments SET phase=? WHERE id=? AND status='active'")
                .bind(INTAKE_PHASE_CONTEXT_REVIEW)
                .bind(assignment.id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            sqlx::query(
                "UPDATE assignment_intakes
                 SET understanding='',constraints_json='{}',plan_json='[]',questions_json='[]',
                     unresolved_questions_json='[]',interview_status='not_started',
                     conversation_state=?,human_response=NULL,approved_by_actor_id=NULL,approved_at=NULL,
                     interview_started_at=NULL,final_summary_message_id=NULL,
                     confirmation_message_id=NULL,converged_at=NULL
                 WHERE assignment_id=?",
            )
            .bind(INTERVIEW_STATE_NOT_STARTED)
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

pub(crate) async fn extend_intake_lease_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    assignment_id: Id,
    now: DateTime<Utc>,
) -> Result<(), DomainError> {
    let interview_deadline = now + Duration::hours(24);
    sqlx::query(
        "UPDATE assignments
         SET expires_at=MAX(expires_at,?),renewed_at=?
         WHERE id=? AND status='active'",
    )
    .bind(interview_deadline.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(assignment_id.to_string())
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(())
}

async fn validate_optional_interview_audit_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    session_id: Id,
    task_id: Id,
    agent_id: Id,
    interview_started_at: Option<DateTime<Utc>>,
    final_summary_message_id: Option<Id>,
    confirmation_message_id: Option<Id>,
) -> Result<(), DomainError> {
    let task_id_text = task_id.to_string();
    let agent_id_text = agent_id.to_string();
    let session = sqlx::query("SELECT agent_instance_id,task_id,status FROM sessions WHERE id=?")
        .bind(session_id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("session {session_id}")))?;
    let session_agent: String = session.try_get("agent_instance_id").map_err(storage)?;
    let session_task: Option<String> = session.try_get("task_id").map_err(storage)?;
    let session_status: String = session.try_get("status").map_err(storage)?;
    if session_agent != agent_id_text
        || session_task.as_deref() != Some(task_id_text.as_str())
        || session_status != "open"
    {
        return Err(DomainError::Conflict(
            "human interview Session is not the open Task Session for this Agent".into(),
        ));
    }

    if let Some(final_summary_message_id) = final_summary_message_id {
        let summary = sqlx::query(
            "SELECT author_type,author_agent_instance_id,created_at,recalled_at
             FROM session_messages WHERE id=? AND session_id=?",
        )
        .bind(final_summary_message_id.to_string())
        .bind(session_id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| {
            DomainError::NotFound(format!("session message {final_summary_message_id}"))
        })?;
        let summary_author: String = summary.try_get("author_type").map_err(storage)?;
        let summary_agent: Option<String> = summary
            .try_get("author_agent_instance_id")
            .map_err(storage)?;
        let summary_recalled: Option<String> = summary.try_get("recalled_at").map_err(storage)?;
        if summary_author != "agent"
            || summary_agent.as_deref() != Some(agent_id_text.as_str())
            || summary_recalled.is_some()
        {
            return Err(DomainError::Conflict(
                "optional final_summary_message_id must identify a current Agent-authored message in the interview Session"
                    .into(),
            ));
        }
        if let Some(started_at) = interview_started_at {
            let summary_created: String = summary.try_get("created_at").map_err(storage)?;
            if parse_dt(summary_created)? < started_at {
                return Err(DomainError::Conflict(
                    "optional final_summary_message_id predates the current interview round".into(),
                ));
            }
        }
    }

    if let Some(confirmation_message_id) = confirmation_message_id {
        let confirmation = sqlx::query(
            "SELECT author_type,recalled_at
             FROM session_messages WHERE id=? AND session_id=?",
        )
        .bind(confirmation_message_id.to_string())
        .bind(session_id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| {
            DomainError::NotFound(format!("session message {confirmation_message_id}"))
        })?;
        let confirmation_author: String = confirmation.try_get("author_type").map_err(storage)?;
        let confirmation_recalled: Option<String> =
            confirmation.try_get("recalled_at").map_err(storage)?;
        if confirmation_author != "human" || confirmation_recalled.is_some() {
            return Err(DomainError::Conflict(
                "optional confirmation_message_id must identify a current Human message in the interview Session"
                    .into(),
            ));
        }
    }
    Ok(())
}

pub(super) async fn enforce_execution_phase_tx(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    assignment: &Assignment,
) -> Result<(), DomainError> {
    if assignment.role != "executor" {
        return Ok(());
    }
    crate::task_graph::enforce_gate_conn(tx, assignment.task_id).await?;
    if assignment.phase == INTAKE_PHASE_IMPLEMENTING {
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
    let converged = intake.conversation_state == INTERVIEW_STATE_CONVERGED
        && intake.approved_at.is_some()
        && intake.converged_at.is_some();
    if !converged {
        blockers.push("human_interview_not_converged".into());
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
        interview_session_id: parse_opt_id(row.try_get("interview_session_id").map_err(storage)?)?,
        conversation_state: row.try_get("conversation_state").map_err(storage)?,
        interview_started_at: parse_opt_dt(row.try_get("interview_started_at").map_err(storage)?)?,
        final_summary_message_id: parse_opt_id(
            row.try_get("final_summary_message_id").map_err(storage)?,
        )?,
        confirmation_message_id: parse_opt_id(
            row.try_get("confirmation_message_id").map_err(storage)?,
        )?,
        converged_at: parse_opt_dt(row.try_get("converged_at").map_err(storage)?)?,
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
