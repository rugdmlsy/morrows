use super::*;
use morrows_core::{LsmJobTerminalEvent, RegisterRunJobWait, RunJobWait, RuntimeJobTerminalEvent};

impl Store {
    pub async fn register_run_job_wait(
        &self,
        input: RegisterRunJobWait,
        actor: Id,
    ) -> Result<RunJobWait, DomainError> {
        let machine = required_text("source_machine", &input.source_machine, 160)?;
        let job_id = required_text("job_id", &input.job_id, 200)?;
        let resume_plan = required_text("resume_plan", &input.resume_plan, 4_000)?;
        let reason = required_text("reason", &input.reason, 2_000)?;
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(input.run_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("run {}", input.run_id)))?,
        )?;
        if run.agent_instance_id != actor {
            return Err(DomainError::Conflict(
                "Run belongs to another AgentInstance".into(),
            ));
        }
        if run.status != "running" {
            return Err(DomainError::Conflict(format!("run is {}", run.status)));
        }
        let runtime_scope_id: String =
            sqlx::query_scalar("SELECT runtime_scope_id FROM run_runtime_bindings WHERE run_id=?")
                .bind(input.run_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| {
                    DomainError::Conflict("Run has no active managed runtime binding".into())
                })?;
        let existing = sqlx::query(
            "SELECT * FROM run_job_waits WHERE run_id=? AND status IN ('pending','ready')",
        )
        .bind(input.run_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?;
        if let Some(row) = existing {
            let wait = row_to_run_job_wait(row)?;
            if wait.source_machine == machine
                && wait.job_id == job_id
                && wait.runtime_scope_id == runtime_scope_id
            {
                tx.commit().await.map_err(storage)?;
                return self.get_run_job_wait(wait.id).await;
            }
            return Err(DomainError::Conflict(
                "Run already has a different active job wait".into(),
            ));
        }
        let wait_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO run_job_waits(id,run_id,source_machine,job_id,runtime_scope_id,status,
             resume_plan,reason,registered_at) VALUES(?,?,?,?,?,'pending',?,?,?)",
        )
        .bind(wait_id.to_string())
        .bind(input.run_id.to_string())
        .bind(&machine)
        .bind(&job_id)
        .bind(&runtime_scope_id)
        .bind(&resume_plan)
        .bind(&reason)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query("UPDATE runs SET stop_reason='waiting_for_runtime_job' WHERE id=?")
            .bind(input.run_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        if let Some(event_id) = sqlx::query_scalar::<_, String>(
            "SELECT event_id FROM runtime_job_terminal_events
             WHERE source_machine=? AND job_id=? AND runtime_scope_id=?
             ORDER BY completed_at DESC,event_id DESC LIMIT 1",
        )
        .bind(&machine)
        .bind(&job_id)
        .bind(&runtime_scope_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        {
            activate_wait_tx(&mut tx, wait_id, input.run_id, &event_id, now).await?;
        }
        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor.to_string(),
            "run",
            input.run_id,
            "run.job_wait_registered",
            json!({
                "task_id":run.task_id,
                "wait_id":wait_id,
                "source_machine":machine,
                "job_id":job_id,
                "runtime_scope_id":runtime_scope_id,
                "resume_plan":resume_plan,
                "reason":reason,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_run_job_wait(wait_id).await
    }

    pub async fn ingest_runtime_job_terminal_event(
        &self,
        event: RuntimeJobTerminalEvent,
    ) -> Result<bool, DomainError> {
        validate_terminal_event(&event)?;
        let now = Utc::now();
        let payload = serde_json::to_string(&event).map_err(storage)?;
        let result_json = event
            .result
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(storage)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let Some(runtime_scope_id) = event
            .runtime_scope_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            tx.commit().await.map_err(storage)?;
            return Ok(false);
        };
        let known_scope: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM run_runtime_bindings WHERE runtime_scope_id=?)",
        )
        .bind(runtime_scope_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if !known_scope {
            tx.commit().await.map_err(storage)?;
            return Ok(false);
        }
        let inserted = sqlx::query(
            "INSERT OR IGNORE INTO runtime_job_terminal_events(event_id,job_id,source_machine,
             runtime_scope_id,attempt,status,exit_code,completed_at,terminal_reason,summary_ref,
             result_json,payload_json,received_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(event.event_id.trim())
        .bind(event.job_id.trim())
        .bind(event.source_machine.trim())
        .bind(event.runtime_scope_id.as_deref().map(str::trim))
        .bind(event.attempt)
        .bind(event.status.trim())
        .bind(event.exit_code)
        .bind(event.completed_at.to_rfc3339())
        .bind(event.terminal_reason.trim())
        .bind(event.summary_ref.as_deref().map(str::trim))
        .bind(result_json)
        .bind(payload)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?
        .rows_affected()
            == 1;
        if inserted && let Some(runtime_scope_id) = event.runtime_scope_id.as_deref() {
            let waits = sqlx::query(
                "SELECT id,run_id FROM run_job_waits WHERE source_machine=? AND job_id=?
                 AND runtime_scope_id=? AND status='pending'",
            )
            .bind(event.source_machine.trim())
            .bind(event.job_id.trim())
            .bind(runtime_scope_id.trim())
            .fetch_all(&mut *tx)
            .await
            .map_err(storage)?;
            for row in waits {
                let wait_id = parse_id(row.try_get("id").map_err(storage)?)?;
                let run_id = parse_id(row.try_get("run_id").map_err(storage)?)?;
                activate_wait_tx(&mut tx, wait_id, run_id, event.event_id.trim(), now).await?;
            }
        }
        tx.commit().await.map_err(storage)?;
        Ok(inserted)
    }

    /// Compatibility wrapper for standalone-LSM event producers. New morrow-runtime
    /// producers use RuntimeJobTerminalEvent and the runtime job-event ingress.
    pub async fn ingest_lsm_job_terminal_event(
        &self,
        event: LsmJobTerminalEvent,
    ) -> Result<bool, DomainError> {
        self.ingest_runtime_job_terminal_event(event.into()).await
    }

    pub async fn run_job_wait_resume_candidates(&self) -> Result<Vec<Id>, DomainError> {
        let rows = sqlx::query(
            "SELECT o.id FROM outbox o JOIN run_job_waits w
               ON json_extract(o.payload_json,'$.wait_id')=w.id
             JOIN runs r ON r.id=w.run_id
             WHERE o.topic='run.wait_resume' AND o.status='pending' AND w.status='ready'
               AND r.status='interrupted'
               AND NOT EXISTS(SELECT 1 FROM launch_attempts a WHERE a.run_id=w.run_id
                              AND a.status IN ('queued','starting','running'))
             ORDER BY o.created_at,o.id",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|row| parse_id(row.try_get("id").map_err(storage)?))
            .collect()
    }

    pub async fn get_run_job_wait(&self, id: Id) -> Result<RunJobWait, DomainError> {
        let row = sqlx::query(
            "SELECT w.*,e.summary_ref,e.result_json FROM run_job_waits w
             LEFT JOIN runtime_job_terminal_events e ON e.event_id=w.terminal_event_id WHERE w.id=?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("run job wait {id}")))?;
        row_to_run_job_wait(row)
    }

    pub async fn latest_run_job_wait(&self, run_id: Id) -> Result<Option<RunJobWait>, DomainError> {
        sqlx::query(
            "SELECT w.*,e.summary_ref,e.result_json FROM run_job_waits w
             LEFT JOIN runtime_job_terminal_events e ON e.event_id=w.terminal_event_id
             WHERE w.run_id=? ORDER BY w.registered_at DESC,w.id DESC LIMIT 1",
        )
        .bind(run_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .map(row_to_run_job_wait)
        .transpose()
    }

    pub async fn run_job_wait_for_launch(
        &self,
        run_id: Id,
        launch_attempt_id: Id,
    ) -> Result<Option<RunJobWait>, DomainError> {
        sqlx::query(
            "SELECT w.*,e.summary_ref,e.result_json FROM run_job_waits w
             LEFT JOIN runtime_job_terminal_events e ON e.event_id=w.terminal_event_id
             WHERE w.run_id=? AND w.resume_launch_attempt_id=? LIMIT 1",
        )
        .bind(run_id.to_string())
        .bind(launch_attempt_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .map(row_to_run_job_wait)
        .transpose()
    }
}

async fn activate_wait_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    wait_id: Id,
    run_id: Id,
    event_id: &str,
    now: DateTime<Utc>,
) -> Result<(), DomainError> {
    let event = sqlx::query(
        "SELECT status,terminal_reason FROM runtime_job_terminal_events WHERE event_id=?",
    )
    .bind(event_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    let status: String = event.try_get("status").map_err(storage)?;
    let terminal_reason: String = event.try_get("terminal_reason").map_err(storage)?;
    let resume_mode = if status == "lost" {
        "reconcile"
    } else {
        "continue"
    };
    let changed = sqlx::query(
        "UPDATE run_job_waits SET status='ready',resume_mode=?,terminal_event_id=?,
         terminal_status=?,terminal_reason=?,ready_at=? WHERE id=? AND status='pending'",
    )
    .bind(resume_mode)
    .bind(event_id)
    .bind(&status)
    .bind(&terminal_reason)
    .bind(now.to_rfc3339())
    .bind(wait_id.to_string())
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    if changed.rows_affected() == 1 {
        let task_id: String = sqlx::query_scalar("SELECT task_id FROM runs WHERE id=?")
            .bind(run_id.to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage)?;
        let outbox_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO outbox(id,topic,payload_json,status,created_at)
             VALUES(?,'run.wait_resume',?,'pending',?)",
        )
        .bind(outbox_id.to_string())
        .bind(json!({"wait_id":wait_id,"run_id":run_id,"event_id":event_id,"resume_mode":resume_mode}).to_string())
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            tx,
            "system",
            "runtime-event-ingest",
            "run",
            run_id,
            if resume_mode == "reconcile" { "run.job_wait_reconciliation_ready" } else { "run.job_wait_ready" },
            json!({"task_id":task_id,"wait_id":wait_id,"event_id":event_id,"terminal_status":status,"terminal_reason":terminal_reason}),
            None,
        )
        .await?;
    }
    Ok(())
}

fn validate_terminal_event(event: &RuntimeJobTerminalEvent) -> Result<(), DomainError> {
    required_text("event_id", &event.event_id, 160)?;
    required_text("job_id", &event.job_id, 200)?;
    required_text("source_machine", &event.source_machine, 160)?;
    required_text("terminal_reason", &event.terminal_reason, 2_000)?;
    if event.attempt < 1 {
        return Err(DomainError::InvalidInput("attempt must be positive".into()));
    }
    if !matches!(
        event.status.as_str(),
        "succeeded" | "failed" | "stopped" | "lost"
    ) {
        return Err(DomainError::InvalidInput("invalid terminal status".into()));
    }
    Ok(())
}

fn required_text(name: &str, value: &str, max: usize) -> Result<String, DomainError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > max {
        return Err(DomainError::InvalidInput(format!(
            "{name} must contain 1..={max} characters"
        )));
    }
    Ok(value.to_owned())
}

fn row_to_run_job_wait(row: sqlx::sqlite::SqliteRow) -> Result<RunJobWait, DomainError> {
    let result_json: Option<String> = row.try_get("result_json").unwrap_or(None);
    Ok(RunJobWait {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        run_id: parse_id(row.try_get("run_id").map_err(storage)?)?,
        source_machine: row.try_get("source_machine").map_err(storage)?,
        job_id: row.try_get("job_id").map_err(storage)?,
        runtime_scope_id: row.try_get("runtime_scope_id").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        resume_mode: row.try_get("resume_mode").map_err(storage)?,
        resume_plan: row.try_get("resume_plan").map_err(storage)?,
        reason: row.try_get("reason").map_err(storage)?,
        terminal_event_id: row.try_get("terminal_event_id").map_err(storage)?,
        terminal_status: row.try_get("terminal_status").map_err(storage)?,
        terminal_reason: row.try_get("terminal_reason").map_err(storage)?,
        resume_launch_attempt_id: parse_opt_id(
            row.try_get("resume_launch_attempt_id").map_err(storage)?,
        )?,
        summary_ref: row.try_get("summary_ref").unwrap_or(None),
        result: result_json
            .map(|value| serde_json::from_str(&value).map_err(storage))
            .transpose()?,
        registered_at: parse_dt(row.try_get("registered_at").map_err(storage)?)?,
        ready_at: parse_opt_dt(row.try_get("ready_at").map_err(storage)?)?,
        queued_at: parse_opt_dt(row.try_get("queued_at").map_err(storage)?)?,
    })
}
