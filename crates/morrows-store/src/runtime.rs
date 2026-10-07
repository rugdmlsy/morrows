use super::*;
use morrows_core::{
    AdhocRuntimeBinding, AttachExecutionEvidence, LaunchAttempt, RunExecutionEvidence,
    RunRuntimeBinding,
};

impl Store {
    pub async fn has_active_implementing_executor_run(
        &self,
        agent_id: Id,
    ) -> Result<bool, DomainError> {
        sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1
                FROM assignments a
                JOIN runs r ON r.assignment_id=a.id
                WHERE a.agent_instance_id=?
                  AND a.role='executor'
                  AND a.status='active'
                  AND a.phase='implementing'
                  AND r.status='running'
            )",
        )
        .bind(agent_id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(storage)
    }

    pub async fn adhoc_runtime_binding(
        &self,
        agent_id: Id,
        machine_id: Id,
    ) -> Result<Option<AdhocRuntimeBinding>, DomainError> {
        let row = sqlx::query(
            "SELECT * FROM adhoc_runtime_bindings
             WHERE agent_instance_id=? AND machine_id=?",
        )
        .bind(agent_id.to_string())
        .bind(machine_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        row.map(row_to_adhoc_runtime_binding).transpose()
    }

    pub async fn ensure_adhoc_runtime_binding(
        &self,
        agent_id: Id,
        machine_id: Id,
    ) -> Result<AdhocRuntimeBinding, DomainError> {
        self.get_agent(agent_id).await?;
        self.get_machine(machine_id).await?;
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO adhoc_runtime_bindings(
                agent_instance_id,machine_id,runtime_scope_id,generation,created_at,updated_at
             ) VALUES(?,?,NULL,1,?,?)
             ON CONFLICT(agent_instance_id,machine_id) DO NOTHING",
        )
        .bind(agent_id.to_string())
        .bind(machine_id.to_string())
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        self.adhoc_runtime_binding(agent_id, machine_id)
            .await?
            .ok_or_else(|| DomainError::Storage("ad-hoc runtime binding disappeared".into()))
    }

    pub async fn bind_adhoc_runtime_scope(
        &self,
        agent_id: Id,
        machine_id: Id,
        generation: i64,
        runtime_scope_id: &str,
    ) -> Result<AdhocRuntimeBinding, DomainError> {
        if runtime_scope_id.trim().is_empty() {
            return Err(DomainError::InvalidInput(
                "runtime_scope_id must be nonempty".into(),
            ));
        }
        let now = Utc::now().to_rfc3339();
        let changed = sqlx::query(
            "UPDATE adhoc_runtime_bindings
             SET runtime_scope_id=?,updated_at=?
             WHERE agent_instance_id=? AND machine_id=? AND generation=?
               AND (runtime_scope_id IS NULL OR runtime_scope_id=?)",
        )
        .bind(runtime_scope_id)
        .bind(&now)
        .bind(agent_id.to_string())
        .bind(machine_id.to_string())
        .bind(generation)
        .bind(runtime_scope_id)
        .execute(&self.pool)
        .await
        .map_err(storage)?
        .rows_affected();
        if changed != 1 {
            return Err(DomainError::Conflict(
                "ad-hoc runtime binding changed concurrently".into(),
            ));
        }
        self.adhoc_runtime_binding(agent_id, machine_id)
            .await?
            .ok_or_else(|| DomainError::Storage("ad-hoc runtime binding disappeared".into()))
    }

    pub async fn rotate_adhoc_runtime_binding(
        &self,
        agent_id: Id,
        machine_id: Id,
        expected_generation: i64,
    ) -> Result<AdhocRuntimeBinding, DomainError> {
        let now = Utc::now().to_rfc3339();
        let changed = sqlx::query(
            "UPDATE adhoc_runtime_bindings
             SET runtime_scope_id=NULL,generation=generation+1,updated_at=?
             WHERE agent_instance_id=? AND machine_id=? AND generation=?",
        )
        .bind(&now)
        .bind(agent_id.to_string())
        .bind(machine_id.to_string())
        .bind(expected_generation)
        .execute(&self.pool)
        .await
        .map_err(storage)?
        .rows_affected();
        if changed != 1 {
            return Err(DomainError::Conflict(
                "ad-hoc runtime binding changed concurrently".into(),
            ));
        }
        self.adhoc_runtime_binding(agent_id, machine_id)
            .await?
            .ok_or_else(|| DomainError::Storage("ad-hoc runtime binding disappeared".into()))
    }

    pub async fn authorize_run_runtime_access(
        &self,
        run_id: Id,
        agent_id: Id,
    ) -> Result<(Run, Task), DomainError> {
        let run = self.get_run(run_id).await?;
        if run.agent_instance_id != agent_id {
            return Err(DomainError::Conflict(
                "Run is not owned by the authenticated AgentInstance".into(),
            ));
        }
        if run.status != "running" {
            return Err(DomainError::Conflict(format!("run is {}", run.status)));
        }
        let assignment = self.get_assignment(run.assignment_id).await?;
        if assignment.agent_instance_id != agent_id
            || assignment.role != "executor"
            || assignment.status != "active"
            || assignment.phase != "implementing"
        {
            return Err(DomainError::Conflict(
                "runtime access requires the active implementing executor Assignment".into(),
            ));
        }
        let task = self.get_task(run.task_id).await?;
        Ok((run, task))
    }

    pub async fn run_runtime_provisioning_subject(
        &self,
        run_id: Id,
    ) -> Result<Option<String>, DomainError> {
        sqlx::query_scalar("SELECT subject FROM run_runtime_provisioning WHERE run_id=?")
            .bind(run_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)
    }

    pub async fn has_managed_runtime(&self, run_id: Id) -> Result<bool, DomainError> {
        sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM run_runtime_bindings WHERE run_id=?)
                  OR EXISTS(SELECT 1 FROM run_runtime_provisioning WHERE run_id=?)",
        )
        .bind(run_id.to_string())
        .bind(run_id.to_string())
        .fetch_one(&self.pool)
        .await
        .map_err(storage)
    }

    /// Runs without a binding still own a durable runtime provisioning key. Replaying that key
    /// recovers a runtime scope created before an HTTP response or DB write was lost.
    pub async fn unbound_runtime_runs(&self) -> Result<Vec<Id>, DomainError> {
        let rows = sqlx::query(
            "SELECT r.id FROM runs r JOIN run_runtime_provisioning p ON p.run_id=r.id
             LEFT JOIN run_runtime_bindings b ON b.run_id=r.id
             WHERE b.run_id IS NULL AND r.status IN ('running','interrupted','cancelling','cleanup_pending','handed_off','completed')",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|row| parse_id(row.try_get("id").map_err(storage)?))
            .collect()
    }

    pub async fn attach_run_execution_evidence(
        &self,
        run_id: Id,
        input: AttachExecutionEvidence,
    ) -> Result<RunExecutionEvidence, DomainError> {
        let run = self.get_run(run_id).await?;
        if !matches!(
            input.kind.as_str(),
            "audit_entry" | "audit_range" | "job" | "artifact"
        ) || input.reference.trim().is_empty()
            || input
                .start_seq
                .zip(input.end_seq)
                .is_some_and(|(start, end)| start > end)
        {
            return Err(DomainError::InvalidInput(
                "invalid execution evidence".into(),
            ));
        }
        if let Some(event_id) = input.event_id {
            let row = sqlx::query("SELECT entity_type,entity_id FROM events WHERE id=?")
                .bind(event_id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("event {event_id}")))?;
            let entity_type: String = row.try_get("entity_type").map_err(storage)?;
            let entity_id: String = row.try_get("entity_id").map_err(storage)?;
            if !(entity_type == "run" && entity_id == run_id.to_string()
                || entity_type == "task" && entity_id == run.task_id.to_string())
            {
                return Err(DomainError::Conflict(
                    "Evidence event belongs to another Run or Task".into(),
                ));
            }
        }
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO run_execution_evidence(id,run_id,event_id,kind,lsm_ref,start_seq,end_seq,created_at)
                     VALUES(?,?,?,?,?,?,?,?)")
            .bind(id.to_string()).bind(run_id.to_string())
            .bind(input.event_id.map(|value| value.to_string()))
            .bind(&input.kind).bind(input.reference.trim())
            .bind(input.start_seq).bind(input.end_seq).bind(Utc::now().to_rfc3339())
            .execute(&self.pool).await.map_err(storage)?;
        self.run_execution_evidence(run_id)
            .await?
            .into_iter()
            .find(|item| item.id == id)
            .ok_or_else(|| DomainError::Storage("Evidence disappeared".into()))
    }

    pub async fn run_execution_evidence(
        &self,
        run_id: Id,
    ) -> Result<Vec<RunExecutionEvidence>, DomainError> {
        self.get_run(run_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM run_execution_evidence WHERE run_id=? ORDER BY created_at,id",
        )
        .bind(run_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|row| {
                Ok(RunExecutionEvidence {
                    id: parse_id(row.try_get("id").map_err(storage)?)?,
                    run_id: parse_id(row.try_get("run_id").map_err(storage)?)?,
                    event_id: parse_opt_id(row.try_get("event_id").map_err(storage)?)?,
                    kind: row.try_get("kind").map_err(storage)?,
                    reference: row.try_get("lsm_ref").map_err(storage)?,
                    start_seq: row.try_get("start_seq").map_err(storage)?,
                    end_seq: row.try_get("end_seq").map_err(storage)?,
                    created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
                })
            })
            .collect()
    }

    pub async fn run_runtime_binding(
        &self,
        run_id: Id,
    ) -> Result<Option<RunRuntimeBinding>, DomainError> {
        let row = sqlx::query("SELECT * FROM run_runtime_bindings WHERE run_id=?")
            .bind(run_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?;
        row.map(|row| {
            Ok(RunRuntimeBinding {
                run_id: parse_id(row.try_get("run_id").map_err(storage)?)?,
                runtime_scope_id: row.try_get("runtime_scope_id").map_err(storage)?,
                capability_id: row.try_get("capability_id").map_err(storage)?,
                restart_deadline_at: parse_opt_dt(
                    row.try_get("restart_deadline_at").map_err(storage)?,
                )?,
            })
        })
        .transpose()
    }

    pub async fn bind_run_runtime(
        &self,
        run_id: Id,
        runtime_scope_id: &str,
    ) -> Result<RunRuntimeBinding, DomainError> {
        let run = self.get_run(run_id).await?;
        if !matches!(
            run.status.as_str(),
            "running"
                | "interrupted"
                | "cancelling"
                | "cleanup_pending"
                | "handed_off"
                | "completed"
        ) {
            return Err(DomainError::Conflict(format!("run is {}", run.status)));
        }
        let now = Utc::now().to_rfc3339();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT runtime_scope_id FROM run_runtime_bindings WHERE run_id=?")
                .bind(run_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?;
        if let Some(existing) = existing {
            if existing != runtime_scope_id {
                return Err(DomainError::Conflict(
                    "Run already owns a different runtime scope".into(),
                ));
            }
        } else {
            let deadline: Option<String> = sqlx::query_scalar(
                "SELECT restart_deadline_at FROM run_runtime_provisioning WHERE run_id=?",
            )
            .bind(run_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .flatten();
            sqlx::query("INSERT INTO run_runtime_bindings(run_id,runtime_scope_id,restart_deadline_at,created_at,updated_at) VALUES(?,?,?,?,?)")
                .bind(run_id.to_string()).bind(runtime_scope_id).bind(deadline).bind(&now).bind(&now)
                .execute(&mut *tx).await.map_err(storage)?;
        }
        tx.commit().await.map_err(storage)?;
        self.run_runtime_binding(run_id)
            .await?
            .ok_or_else(|| DomainError::Storage("runtime binding disappeared".into()))
    }

    /// The conditional write is the durable fence between an external runtime capability issue
    /// and a concurrent Run cancellation. SQLite serializes it with cancel's
    /// transaction, so a token issued for a cancelled Run is never adopted.
    pub async fn try_adopt_run_capability(
        &self,
        run_id: Id,
        capability_id: &str,
    ) -> Result<(), DomainError> {
        let result = sqlx::query(
            "UPDATE run_runtime_bindings SET capability_id=?,updated_at=? WHERE run_id=?
             AND EXISTS (SELECT 1 FROM runs r WHERE r.id=run_runtime_bindings.run_id
                         AND r.status='running')",
        )
        .bind(capability_id)
        .bind(Utc::now().to_rfc3339())
        .bind(run_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        if result.rows_affected() != 1 {
            return Err(DomainError::Conflict(
                "Run stopped before its runtime capability could be adopted".into(),
            ));
        }
        Ok(())
    }

    /// A revoke for an older launch attempt must not clear a newer token.
    pub async fn clear_run_capability_if_matches(
        &self,
        run_id: Id,
        capability_id: &str,
    ) -> Result<(), DomainError> {
        sqlx::query(
            "UPDATE run_runtime_bindings SET capability_id=NULL,updated_at=?
             WHERE run_id=? AND capability_id=?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(run_id.to_string())
        .bind(capability_id)
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(())
    }

    /// Explicit restart creates a new process attempt while preserving the Run and
    /// its runtime scope. The transaction prevents two restarts claiming one window.
    pub async fn enqueue_run_restart(&self, run_id: Id) -> Result<LaunchAttempt, DomainError> {
        self.enqueue_run_restart_with_actor(run_id, "human", "local", "run.restart_requested", None)
            .await
    }

    pub async fn enqueue_run_delivery_resume(
        &self,
        run_id: Id,
    ) -> Result<LaunchAttempt, DomainError> {
        self.enqueue_run_restart_with_actor(
            run_id,
            "system",
            "delivery",
            "run.delivery_resume_requested",
            None,
        )
        .await
    }

    pub async fn enqueue_run_job_wait_resume(
        &self,
        outbox_id: Id,
    ) -> Result<LaunchAttempt, DomainError> {
        let row = sqlx::query(
            "SELECT w.run_id FROM outbox o JOIN run_job_waits w
               ON json_extract(o.payload_json,'$.wait_id')=w.id
             WHERE o.id=? AND o.topic='run.wait_resume' AND o.status='pending' AND w.status='ready'",
        )
        .bind(outbox_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::Conflict("wait resume is no longer pending".into()))?;
        let run_id = parse_id(row.try_get("run_id").map_err(storage)?)?;
        self.enqueue_run_restart_with_actor(
            run_id,
            "system",
            "runtime-job-event",
            "run.job_wait_resume_requested",
            Some(outbox_id),
        )
        .await
    }

    async fn enqueue_run_restart_with_actor(
        &self,
        run_id: Id,
        actor_type: &str,
        actor_id: &str,
        event_type: &str,
        wait_outbox_id: Option<Id>,
    ) -> Result<LaunchAttempt, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(run_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("run {run_id}")))?,
        )?;
        if run.status != "interrupted" {
            return Err(DomainError::Conflict(format!("run is {}", run.status)));
        }
        let deadline = if wait_outbox_id.is_some() {
            let deadline =
                now + chrono::Duration::seconds(crate::launch::agent_restart_grace_seconds());
            sqlx::query(
                "UPDATE run_runtime_bindings SET restart_deadline_at=?,updated_at=? WHERE run_id=?",
            )
            .bind(deadline.to_rfc3339())
            .bind(now.to_rfc3339())
            .bind(run_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            sqlx::query("UPDATE assignments SET expires_at=MAX(expires_at,?),renewed_at=? WHERE id=? AND status='active'")
                .bind(deadline.to_rfc3339())
                .bind(now.to_rfc3339())
                .bind(run.assignment_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            deadline
        } else {
            let deadline: String = sqlx::query_scalar(
                "SELECT restart_deadline_at FROM run_runtime_bindings WHERE run_id=?",
            )
            .bind(run_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .flatten()
            .ok_or_else(|| DomainError::Conflict("Run has no restart deadline".into()))?;
            parse_dt(deadline)?
        };
        if deadline <= now {
            return Err(DomainError::Conflict(
                "Run restart window has expired".into(),
            ));
        }
        let assignment_active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM assignments WHERE id=? AND status='active' AND expires_at>?)",
        )
        .bind(run.assignment_id.to_string()).bind(now.to_rfc3339())
        .fetch_one(&mut *tx).await.map_err(storage)?;
        if !assignment_active {
            return Err(DomainError::Conflict("Run Assignment is not active".into()));
        }
        let active_launch: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM launch_attempts WHERE run_id=?
             AND status IN ('queued','starting','running'))",
        )
        .bind(run_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if active_launch {
            return Err(DomainError::Conflict(
                "Run already has an active launch attempt".into(),
            ));
        }
        let previous = sqlx::query(
            "SELECT id,launch_profile_id,cwd FROM launch_attempts
             WHERE run_id=? ORDER BY created_at DESC,id DESC LIMIT 1",
        )
        .bind(run_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::Conflict("Run has no launch attempt".into()))?;
        let previous_id: String = previous.try_get("id").map_err(storage)?;
        let profile_id: String = previous.try_get("launch_profile_id").map_err(storage)?;
        let cwd: Option<String> = previous.try_get("cwd").map_err(storage)?;
        let resume_attempt_id = run.provider_conversation_ref.as_ref().map(|_| previous_id);
        let attempt_id = Uuid::new_v4();
        let job_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO jobs(id,kind,payload_json,status,available_at,attempt_count,created_at)
                     VALUES(?,'launch_executor',?,'pending',?,0,?)",
        )
        .bind(job_id.to_string())
        .bind(json!({"launch_attempt_id":attempt_id}).to_string())
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query("INSERT INTO launch_attempts(id,assignment_id,task_id,agent_instance_id,launch_profile_id,
                     job_id,resume_from_attempt_id,restart_run_id,status,cwd,created_at)
                     VALUES(?,?,?,?,?,?,?,?,'queued',?,?)")
            .bind(attempt_id.to_string()).bind(run.assignment_id.to_string())
            .bind(run.task_id.to_string()).bind(run.agent_instance_id.to_string())
            .bind(profile_id).bind(job_id.to_string())
            .bind(resume_attempt_id).bind(run_id.to_string())
            .bind(cwd).bind(now.to_rfc3339()).execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(
            &mut tx,
            actor_type,
            actor_id,
            "run",
            run_id,
            event_type,
            json!({"launch_attempt_id":attempt_id}),
            None,
        )
        .await?;
        if let Some(outbox_id) = wait_outbox_id {
            let wait_id: String = sqlx::query_scalar(
                "SELECT json_extract(payload_json,'$.wait_id') FROM outbox
                 WHERE id=? AND topic='run.wait_resume' AND status='pending'",
            )
            .bind(outbox_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::Conflict("wait resume is no longer pending".into()))?;
            sqlx::query("UPDATE run_job_waits SET status='queued',queued_at=?,resume_launch_attempt_id=? WHERE id=? AND status='ready'")
                .bind(now.to_rfc3339())
                .bind(attempt_id.to_string())
                .bind(&wait_id)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            sqlx::query("UPDATE outbox SET status='delivered',delivered_at=? WHERE id=? AND status='pending'")
                .bind(now.to_rfc3339())
                .bind(outbox_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
        }
        tx.commit().await.map_err(storage)?;
        self.get_launch_attempt(attempt_id).await
    }

    pub async fn request_run_cancel(&self, run_id: Id) -> Result<Run, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(run_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("run {run_id}")))?,
        )?;
        if !matches!(
            run.status.as_str(),
            "running" | "interrupted" | "cancelling"
        ) {
            return Err(DomainError::Conflict(format!("run is {}", run.status)));
        }
        let lsm_runtime: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM run_runtime_bindings WHERE run_id=?)
                  OR EXISTS(SELECT 1 FROM run_runtime_provisioning WHERE run_id=?)",
        )
        .bind(run_id.to_string())
        .bind(run_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if !lsm_runtime {
            return Err(DomainError::Conflict(
                "Run has no managed runtime provisioning intent".into(),
            ));
        }
        if run.status == "cancelling" {
            tx.commit().await.map_err(storage)?;
            return Ok(run);
        }
        sqlx::query("UPDATE runs SET status='cancelling',stop_reason='user_cancelled' WHERE id=?")
            .bind(run_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "human",
            "local",
            "run",
            run_id,
            "run.cancelling",
            json!({"requested_at":now}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_run(run_id).await
    }

    pub async fn due_interrupted_runs(&self) -> Result<Vec<Id>, DomainError> {
        let rows = sqlx::query(
            "SELECT r.id FROM runs r
             LEFT JOIN run_runtime_bindings b ON b.run_id=r.id
             LEFT JOIN run_runtime_provisioning p ON p.run_id=r.id
             WHERE r.status='interrupted'
               AND COALESCE(b.restart_deadline_at,p.restart_deadline_at)<=?",
        )
        .bind(Utc::now().to_rfc3339())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|row| parse_id(row.try_get("id").map_err(storage)?))
            .collect()
    }

    pub async fn interrupted_runtime_runs(&self) -> Result<Vec<Id>, DomainError> {
        let rows = sqlx::query(
            "SELECT r.id FROM runs r JOIN run_runtime_bindings b ON b.run_id=r.id
                                WHERE r.status='interrupted' AND b.capability_id IS NOT NULL",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|row| parse_id(row.try_get("id").map_err(storage)?))
            .collect()
    }

    pub async fn completed_runtime_runs(&self) -> Result<Vec<Id>, DomainError> {
        let rows = sqlx::query(
            "SELECT r.id FROM runs r JOIN run_runtime_bindings b ON b.run_id=r.id
                                WHERE r.status IN ('completed','handed_off') AND b.scope_terminalized_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|row| parse_id(row.try_get("id").map_err(storage)?))
            .collect()
    }

    pub async fn mark_runtime_scope_terminalized(&self, run_id: Id) -> Result<(), DomainError> {
        sqlx::query(
            "UPDATE run_runtime_bindings SET scope_terminalized_at=?,updated_at=? WHERE run_id=?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .bind(run_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(())
    }

    pub async fn has_active_launch_attempt(&self, run_id: Id) -> Result<bool, DomainError> {
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM launch_attempts WHERE run_id=? AND status IN ('starting','running'))")
            .bind(run_id.to_string()).fetch_one(&self.pool).await.map_err(storage)
    }

    pub async fn begin_expired_cleanup(&self, run_id: Id) -> Result<(), DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(run_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?,
        )?;
        if run.status != "interrupted" {
            return Ok(());
        }
        let deadline: Option<String> = sqlx::query_scalar(
            "SELECT COALESCE(b.restart_deadline_at,p.restart_deadline_at)
                 FROM runs r LEFT JOIN run_runtime_bindings b ON b.run_id=r.id
                 LEFT JOIN run_runtime_provisioning p ON p.run_id=r.id WHERE r.id=?",
        )
        .bind(run_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .flatten();
        if deadline
            .as_deref()
            .is_none_or(|value| value > now.to_rfc3339().as_str())
        {
            return Err(DomainError::Conflict(
                "Run restart window has not expired".into(),
            ));
        }
        sqlx::query("UPDATE runs SET status='cleanup_pending',stop_reason='agent_restart_window_expired' WHERE id=?")
            .bind(run_id.to_string()).execute(&mut *tx).await.map_err(storage)?;
        sqlx::query("UPDATE assignments SET status='released',released_at=?,release_reason='agent_restart_window_expired'
                     WHERE id=? AND status='active'")
            .bind(now.to_rfc3339()).bind(run.assignment_id.to_string())
            .execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(
            &mut tx,
            "system",
            "launcher",
            "run",
            run_id,
            "run.cleanup_pending",
            json!({"reason":"agent_restart_window_expired"}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(())
    }

    pub async fn pending_runtime_cleanup_runs(&self) -> Result<Vec<Id>, DomainError> {
        let rows = sqlx::query(
            "SELECT r.id FROM runs r
             LEFT JOIN run_runtime_bindings b ON b.run_id=r.id
             LEFT JOIN run_runtime_provisioning p ON p.run_id=r.id
             WHERE r.status IN ('cleanup_pending','cancelling')
               AND (b.run_id IS NOT NULL OR p.run_id IS NOT NULL)",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|row| parse_id(row.try_get("id").map_err(storage)?))
            .collect()
    }

    pub async fn complete_runtime_cleanup(&self, run_id: Id) -> Result<Run, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(run_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?,
        )?;
        let final_status = match run.status.as_str() {
            "cleanup_pending" => "failed",
            "cancelling" => "cancelled",
            other => return Err(DomainError::Conflict(format!("run is {other}"))),
        };
        sqlx::query("UPDATE runs SET status=?,ended_at=? WHERE id=?")
            .bind(final_status)
            .bind(now.to_rfc3339())
            .bind(run_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        sqlx::query(
            "UPDATE run_runtime_bindings SET scope_terminalized_at=?,updated_at=? WHERE run_id=?",
        )
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(run_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE assignments SET status='released',released_at=?,release_reason=?
                     WHERE id=? AND status='active'",
        )
        .bind(now.to_rfc3339())
        .bind(final_status)
        .bind(run.assignment_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE tasks SET state='ready',updated_at=? WHERE id=? AND state='in_progress'",
        )
        .bind(now.to_rfc3339())
        .bind(run.task_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "system",
            "launcher",
            "run",
            run_id,
            if final_status == "failed" {
                "run.failed"
            } else {
                "run.cancelled"
            },
            json!({"reason":run.stop_reason}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_run(run_id).await
    }

    pub async fn renew_interrupted_assignments(&self) -> Result<(), DomainError> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "UPDATE assignments SET expires_at=MAX(expires_at,b.restart_deadline_at),renewed_at=?
                     FROM run_runtime_bindings b JOIN runs r ON r.id=b.run_id
                     WHERE assignments.id=r.assignment_id AND r.status='interrupted'
                       AND b.restart_deadline_at> ? AND assignments.status='active'",
        )
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE assignments SET expires_at=MAX(expires_at,p.restart_deadline_at),renewed_at=?
             FROM run_runtime_provisioning p JOIN runs r ON r.id=p.run_id
             LEFT JOIN run_runtime_bindings b ON b.run_id=r.id
             WHERE assignments.id=r.assignment_id AND r.status='interrupted'
               AND b.run_id IS NULL AND p.restart_deadline_at>?
               AND assignments.status='active'",
        )
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(())
    }
}

fn row_to_adhoc_runtime_binding(
    row: sqlx::sqlite::SqliteRow,
) -> Result<AdhocRuntimeBinding, DomainError> {
    Ok(AdhocRuntimeBinding {
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        machine_id: parse_id(row.try_get("machine_id").map_err(storage)?)?,
        runtime_scope_id: row.try_get("runtime_scope_id").map_err(storage)?,
        generation: row.try_get("generation").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}
