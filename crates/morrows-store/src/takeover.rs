use super::*;
use morrows_core::TaskClaim;

impl Store {
    /// Advisory status only; takeover revalidates under an IMMEDIATE transaction.
    pub async fn task_takeover_status(&self, task_id: Id) -> Result<Value, DomainError> {
        let mut conn = self.pool.acquire().await.map_err(storage)?;
        let row = sqlx::query("SELECT state,assignment_mode FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_optional(&mut *conn)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?;
        let state: String = row.try_get("state").map_err(storage)?;
        let mode: String = row.try_get("assignment_mode").map_err(storage)?;
        let previous = latest_executor(&mut conn, task_id).await?;
        let owner_live = previous
            .as_ref()
            .is_some_and(|a| a.status == "active" && a.expires_at > Utc::now());
        let blocked = unsafe_predecessor(&mut conn, task_id).await?;
        let eligible = state == "in_progress" && !owner_live && blocked.is_none();
        let last_run:Option<String>=sqlx::query_scalar("SELECT r.id FROM runs r JOIN assignments a ON a.id=r.assignment_id WHERE r.task_id=? AND a.role='executor' ORDER BY r.started_at DESC,r.rowid DESC LIMIT 1")
            .bind(task_id.to_string()).fetch_optional(&mut *conn).await.map_err(storage)?;
        let latest_milestone_summary: Option<String> = sqlx::query_scalar(
            "SELECT json_extract(content_json,'$.summary') FROM run_milestones WHERE run_id=? ORDER BY sequence DESC LIMIT 1")
            .bind(last_run.as_deref()).fetch_optional(&mut *conn).await.map_err(storage)?;
        let reason = if state != "in_progress" {
            format!("task is {state}")
        } else if owner_live {
            "active executor lease".into()
        } else if let Some(s) = blocked {
            s
        } else {
            "eligible for authenticated takeover".into()
        };
        Ok(
            json!({"task_id":task_id,"task_state":state,"assignment_mode":mode,
            "eligible":eligible,"employee_self_takeover":eligible && mode=="open",
            "reason":reason,"latest_executor_run_id":last_run,"latest_executor_milestone_summary":latest_milestone_summary,
            "current_executor":previous.as_ref().map(|a|json!({"assignment_id":a.id,"agent_instance_id":a.agent_instance_id,
                "status":a.status,"phase":a.phase,"expires_at":a.expires_at}))}),
        )
    }

    /// Legacy call signature for internal callers; use takeover_task_request for retry safety.
    pub async fn takeover_task(
        &self,
        task_id: Id,
        successor: Id,
        lease_seconds: i64,
        reason: &str,
        force: bool,
        operator_actor: Option<&str>,
    ) -> Result<TaskClaim, DomainError> {
        self.takeover_task_request(
            task_id,
            successor,
            lease_seconds,
            reason,
            force,
            operator_actor,
            None,
        )
        .await
    }

    /// CAS transfer, with a fresh intake Run; never rebind the predecessor Run.
    pub async fn takeover_task_request(
        &self,
        task_id: Id,
        successor: Id,
        lease_seconds: i64,
        reason: &str,
        force: bool,
        operator_actor: Option<&str>,
        request_id: Option<&str>,
    ) -> Result<TaskClaim, DomainError> {
        self.get_agent(successor).await?;
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > 1000 {
            return Err(DomainError::InvalidInput(
                "takeover reason must contain 1..1000 characters".into(),
            ));
        }
        if force && operator_actor.is_none() {
            return Err(DomainError::InvalidInput(
                "force takeover requires authenticated operator".into(),
            ));
        }
        let key = request_id
            .map(str::to_owned)
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        if key.trim().is_empty() || key.len() > 128 {
            return Err(DomainError::InvalidInput(
                "takeover request_id must be 1..128 bytes".into(),
            ));
        }
        let actor = operator_actor
            .map(str::to_owned)
            .unwrap_or_else(|| successor.to_string());
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        if let Some(row)=sqlx::query("SELECT task_id,successor_agent_instance_id,reason,force,assignment_id,run_id FROM executor_takeover_requests WHERE actor=? AND idempotency_key=?")
            .bind(&actor).bind(&key).fetch_optional(&mut *tx).await.map_err(storage)? {
            let prior_task:String=row.try_get("task_id").map_err(storage)?;
            let prior_successor:String=row.try_get("successor_agent_instance_id").map_err(storage)?;
            let prior_reason:String=row.try_get("reason").map_err(storage)?;
            let prior_force:i64=row.try_get("force").map_err(storage)?;
            if prior_task != task_id.to_string() || prior_successor != successor.to_string()
                || prior_reason != reason || prior_force != i64::from(force) {
                return Err(DomainError::Conflict("takeover request_id reused with a different payload".into()));
            }
            let aid=parse_id(row.try_get::<String,_>("assignment_id").map_err(storage)?)?;
            let rid=parse_id(row.try_get::<String,_>("run_id").map_err(storage)?)?;
            tx.commit().await.map_err(storage)?;
            return Ok(TaskClaim{assignment:self.get_assignment(aid).await?,
                run:self.get_run(rid).await?});
        }
        let row = sqlx::query("SELECT state,assignment_mode FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?;
        let state: String = row.try_get("state").map_err(storage)?;
        let mode: String = row.try_get("assignment_mode").map_err(storage)?;
        if state != "in_progress" {
            return Err(DomainError::Conflict(format!(
                "task is {state}; takeover needs in_progress"
            )));
        }
        if !force && mode != "open" {
            return Err(DomainError::Conflict(format!(
                "{mode} tasks require authorized control-plane assignment"
            )));
        }
        if let Some(reason) = unsafe_predecessor(&mut tx, task_id).await? {
            return Err(DomainError::Conflict(format!("takeover_blocked: {reason}")));
        }
        let previous = latest_executor(&mut tx, task_id).await?;
        let now = Utc::now();
        if previous
            .as_ref()
            .is_some_and(|a| a.status == "active" && a.expires_at > now)
            && !force
        {
            return Err(DomainError::Conflict(
                "active executor lease; ordinary agents cannot preempt".into(),
            ));
        }
        let old_runs:Vec<String>=sqlx::query_scalar(
            "SELECT r.id FROM runs r JOIN assignments a ON r.assignment_id=a.id WHERE r.task_id=? AND a.role='executor' AND r.status IN ('running','paused')")
            .bind(task_id.to_string()).fetch_all(&mut *tx).await.map_err(storage)?;
        let stamp = now.to_rfc3339();
        sqlx::query("UPDATE runs SET status='abandoned',stop_reason='takeover',ended_at=? WHERE task_id=? AND status IN ('running','paused') AND assignment_id IN (SELECT id FROM assignments WHERE task_id=? AND role='executor')")
            .bind(&stamp).bind(task_id.to_string()).bind(task_id.to_string()).execute(&mut *tx).await.map_err(storage)?;
        sqlx::query("UPDATE assignments SET status='released',released_at=?,release_reason='takeover' WHERE task_id=? AND role='executor' AND status IN ('active','expired')")
            .bind(&stamp).bind(task_id.to_string()).execute(&mut *tx).await.map_err(storage)?;
        sqlx::query(
            "UPDATE tasks SET state='ready',updated_at=? WHERE id=? AND state='in_progress'",
        )
        .bind(&stamp)
        .bind(task_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        let assignment =
            super::claim_task_tx(&mut tx, task_id, successor, "executor", lease_seconds, true)
                .await?;
        let run = super::start_run_tx(self, &mut tx, assignment.id, successor, None, false).await?;
        sqlx::query("INSERT INTO executor_takeover_requests(actor,idempotency_key,task_id,successor_agent_instance_id,reason,force,assignment_id,run_id,created_at)
            VALUES(?,?,?,?,?,?,?,?,?)")
            .bind(&actor).bind(&key).bind(task_id.to_string()).bind(successor.to_string())
            .bind(reason).bind(force).bind(assignment.id.to_string()).bind(run.id.to_string())
            .bind(&stamp).execute(&mut *tx).await.map_err(storage)?;
        let actor_type = if force { "operator" } else { "agent_instance" };
        append_event_tx(&mut tx,actor_type,&actor,"task",task_id,"task.executor_taken_over",
            json!({"previous_assignment":previous.as_ref().map(|a|a.id),"previous_agent":previous.as_ref().map(|a|a.agent_instance_id),
              "previous_run_ids":old_runs,"new_assignment_id":assignment.id,"new_run_id":run.id,
              "successor":successor,"reason":reason,"force":force,"request_id":key}),None).await?;
        tx.commit().await.map_err(storage)?;
        Ok(TaskClaim { assignment, run })
    }
}

async fn latest_executor(
    conn: &mut sqlx::SqliteConnection,
    task_id: Id,
) -> Result<Option<Assignment>, DomainError> {
    sqlx::query("SELECT * FROM assignments WHERE task_id=? AND role='executor' ORDER BY acquired_at DESC,rowid DESC LIMIT 1")
        .bind(task_id.to_string()).fetch_optional(conn).await.map_err(storage)?
        .map(row_to_assignment).transpose()
}

/// An expired lease is not proof a remote process has stopped.
async fn unsafe_predecessor(
    conn: &mut sqlx::SqliteConnection,
    task_id: Id,
) -> Result<Option<String>, DomainError> {
    if super::continuation::predecessor_runtime_active(conn, task_id).await? {
        return Ok(Some(
            "managed launch or RuntimeScope is still active".into(),
        ));
    }
    let blocked:bool=sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE task_id=? AND status IN ('interrupted','cleanup_pending','cancelling'))
         OR EXISTS(SELECT 1 FROM runs r JOIN run_runtime_bindings b ON b.run_id=r.id WHERE r.task_id=? AND b.scope_terminalized_at IS NULL)
         OR EXISTS(SELECT 1 FROM runs r JOIN run_runtime_provisioning p ON p.run_id=r.id LEFT JOIN run_runtime_bindings b ON b.run_id=r.id WHERE r.task_id=? AND b.run_id IS NULL)
         OR EXISTS(SELECT 1 FROM run_job_waits w JOIN runs r ON r.id=w.run_id WHERE r.task_id=? AND w.status IN ('pending','ready','queued'))
         OR EXISTS(SELECT 1 FROM launch_attempts WHERE task_id=? AND status IN ('starting','running'))")
        .bind(task_id.to_string()).bind(task_id.to_string()).bind(task_id.to_string())
        .bind(task_id.to_string()).bind(task_id.to_string()).fetch_one(conn).await.map_err(storage)?;
    Ok(blocked.then(|| "prior runtime, job or cleanup has not quiesced".into()))
}
