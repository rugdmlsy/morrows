use super::*;
use ac_core::*;
use std::path::Path;

impl Store {
    pub async fn register_launch_profile(
        &self,
        mut input: RegisterLaunchProfile,
    ) -> Result<LaunchProfile, DomainError> {
        input.name = input.name.trim().to_owned();
        input.adapter = input.adapter.trim().to_owned();
        input.program = input.program.trim().to_owned();
        input.codex_home = trim_optional(input.codex_home);
        input.default_cwd = trim_optional(input.default_cwd);
        input.model = trim_optional(input.model);
        if input.name.is_empty() {
            return Err(DomainError::InvalidInput(
                "launch profile name cannot be empty".into(),
            ));
        }
        if input.adapter != "codex_cli" {
            return Err(DomainError::InvalidInput(format!(
                "unsupported launch adapter {}",
                input.adapter
            )));
        }
        validate_absolute("program", &input.program)?;
        if let Some(value) = &input.codex_home {
            validate_absolute("codex_home", value)?;
        }
        if let Some(value) = &input.default_cwd {
            validate_absolute("default_cwd", value)?;
        }
        self.get_agent(input.agent_instance_id).await?;

        let now = Utc::now();
        let id = Uuid::new_v4();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO launch_profiles(id,name,adapter,agent_instance_id,program,codex_home,default_cwd,model,enabled,metadata_json,created_at,updated_at)
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(&input.name)
        .bind(&input.adapter)
        .bind(input.agent_instance_id.to_string())
        .bind(&input.program)
        .bind(&input.codex_home)
        .bind(&input.default_cwd)
        .bind(&input.model)
        .bind(input.enabled)
        .bind(input.metadata.to_string())
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "system",
            "launcher",
            "agent_instance",
            input.agent_instance_id,
            "launch.profile_registered",
            json!({"launch_profile_id":id,"adapter":input.adapter,"name":input.name}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_launch_profile(id).await
    }

    pub async fn list_launch_profiles(&self) -> Result<Vec<LaunchProfile>, DomainError> {
        let rows = sqlx::query("SELECT * FROM launch_profiles ORDER BY created_at,id")
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        rows.into_iter().map(row_to_launch_profile).collect()
    }

    pub async fn get_launch_profile(&self, id: Id) -> Result<LaunchProfile, DomainError> {
        let row = sqlx::query("SELECT * FROM launch_profiles WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("launch profile {id}")))?;
        row_to_launch_profile(row)
    }

    pub async fn enqueue_launch(&self, input: EnqueueLaunch) -> Result<LaunchAttempt, DomainError> {
        if let Some(cwd) = &input.cwd {
            validate_absolute("cwd", cwd)?;
        }
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let assignment = row_to_assignment(
            sqlx::query("SELECT * FROM assignments WHERE id=?")
                .bind(input.assignment_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| {
                    DomainError::NotFound(format!("assignment {}", input.assignment_id))
                })?,
        )?;
        if assignment.role != "executor" {
            return Err(DomainError::Conflict(
                "only executor assignments can be launched".into(),
            ));
        }
        if assignment.status != "active" || assignment.expires_at <= now {
            return Err(DomainError::Conflict("assignment is not active".into()));
        }
        let profile = row_to_launch_profile(
            sqlx::query("SELECT * FROM launch_profiles WHERE id=?")
                .bind(input.launch_profile_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| {
                    DomainError::NotFound(format!("launch profile {}", input.launch_profile_id))
                })?,
        )?;
        if !profile.enabled {
            return Err(DomainError::Conflict("launch profile is disabled".into()));
        }
        if profile.agent_instance_id != assignment.agent_instance_id {
            return Err(DomainError::Conflict(
                "launch profile belongs to another agent instance".into(),
            ));
        }
        let cwd = profile.default_cwd.clone().ok_or_else(|| {
            DomainError::InvalidInput(
                "launch profile must define an operator-controlled default_cwd".into(),
            )
        })?;
        if let Some(requested) = input.cwd.as_deref()
            && requested != cwd
        {
            return Err(DomainError::Conflict(
                "launch cwd override is outside the launch profile contract".into(),
            ));
        }
        validate_absolute("cwd", &cwd)?;
        if !Path::new(&cwd).is_dir() {
            return Err(DomainError::InvalidInput(format!(
                "launch cwd does not exist or is not a directory: {cwd}"
            )));
        }
        let already_active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM launch_attempts WHERE assignment_id=? AND status IN ('queued','starting','running'))",
        )
        .bind(assignment.id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if already_active {
            return Err(DomainError::Conflict(
                "assignment already has an active launch attempt".into(),
            ));
        }

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
        sqlx::query(
            "INSERT INTO launch_attempts(id,assignment_id,task_id,agent_instance_id,launch_profile_id,job_id,status,cwd,created_at)
             VALUES(?,?,?,?,?,?,'queued',?,?)",
        )
        .bind(attempt_id.to_string())
        .bind(assignment.id.to_string())
        .bind(assignment.task_id.to_string())
        .bind(assignment.agent_instance_id.to_string())
        .bind(profile.id.to_string())
        .bind(job_id.to_string())
        .bind(&cwd)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "system",
            "launcher",
            "task",
            assignment.task_id,
            "launch.queued",
            json!({"launch_attempt_id":attempt_id,"assignment_id":assignment.id,"launch_profile_id":profile.id,"job_id":job_id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_launch_attempt(attempt_id).await
    }

    pub async fn get_launch_attempt(&self, id: Id) -> Result<LaunchAttempt, DomainError> {
        let row = sqlx::query("SELECT * FROM launch_attempts WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("launch attempt {id}")))?;
        row_to_launch_attempt(row)
    }

    pub async fn task_launch_attempts(
        &self,
        task_id: Id,
    ) -> Result<Vec<LaunchAttempt>, DomainError> {
        self.get_task(task_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM launch_attempts WHERE task_id=? ORDER BY created_at DESC,id DESC",
        )
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_launch_attempt).collect()
    }

    pub async fn claim_launch_job(&self) -> Result<Option<ClaimedLaunchJob>, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row = sqlx::query(
            "SELECT j.id AS job_id,a.id AS attempt_id
             FROM jobs j JOIN launch_attempts a ON a.job_id=j.id
             WHERE j.kind='launch_executor' AND j.status='pending' AND j.available_at<=? AND a.status='queued'
             ORDER BY j.available_at,j.created_at,j.id LIMIT 1",
        )
        .bind(now.to_rfc3339())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?;
        let Some(row) = row else {
            tx.commit().await.map_err(storage)?;
            return Ok(None);
        };
        let job_id = parse_id(row.try_get("job_id").map_err(storage)?)?;
        let attempt_id = parse_id(row.try_get("attempt_id").map_err(storage)?)?;
        let updated = sqlx::query(
            "UPDATE jobs SET status='running',claimed_at=?,attempt_count=attempt_count+1 WHERE id=? AND status='pending'",
        )
        .bind(now.to_rfc3339())
        .bind(job_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        if updated.rows_affected() != 1 {
            tx.rollback().await.map_err(storage)?;
            return Ok(None);
        }
        tx.commit().await.map_err(storage)?;
        let attempt = self.get_launch_attempt(attempt_id).await?;
        let profile = self.get_launch_profile(attempt.launch_profile_id).await?;
        Ok(Some(ClaimedLaunchJob {
            job_id,
            attempt,
            profile,
        }))
    }

    pub async fn begin_launch_attempt(&self, id: Id) -> Result<LaunchExecution, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let attempt = row_to_launch_attempt(
            sqlx::query("SELECT * FROM launch_attempts WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("launch attempt {id}")))?,
        )?;
        if attempt.status != "queued" {
            return Err(DomainError::Conflict(format!(
                "launch attempt is {}",
                attempt.status
            )));
        }
        let assignment = row_to_assignment(
            sqlx::query("SELECT * FROM assignments WHERE id=?")
                .bind(attempt.assignment_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound("assignment".into()))?,
        )?;
        if assignment.status != "active" || assignment.expires_at <= now {
            return Err(DomainError::Conflict("assignment is not active".into()));
        }
        if assignment.agent_instance_id != attempt.agent_instance_id {
            return Err(DomainError::Conflict(
                "launch attempt agent mismatch".into(),
            ));
        }
        let profile = row_to_launch_profile(
            sqlx::query("SELECT * FROM launch_profiles WHERE id=?")
                .bind(attempt.launch_profile_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound("launch profile".into()))?,
        )?;
        if !profile.enabled {
            return Err(DomainError::Conflict("launch profile is disabled".into()));
        }
        if profile.agent_instance_id != assignment.agent_instance_id {
            return Err(DomainError::Conflict(
                "launch profile agent mismatch".into(),
            ));
        }
        let running: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE assignment_id=? AND status IN ('running','paused'))",
        )
        .bind(assignment.id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if running {
            return Err(DomainError::Conflict(
                "assignment already has an active run".into(),
            ));
        }

        let run_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO runs(id,task_id,assignment_id,agent_instance_id,status,started_at)
             VALUES(?,?,?,?,'running',?)",
        )
        .bind(run_id.to_string())
        .bind(assignment.task_id.to_string())
        .bind(assignment.id.to_string())
        .bind(assignment.agent_instance_id.to_string())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE launch_attempts SET status='starting',run_id=?,started_at=? WHERE id=? AND status='queued'",
        )
        .bind(run_id.to_string())
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "system",
            "launcher",
            "run",
            run_id,
            "run.started",
            json!({"task_id":assignment.task_id,"assignment_id":assignment.id,"launch_attempt_id":id}),
            None,
        )
        .await?;
        append_event_tx(
            &mut tx,
            "system",
            "launcher",
            "task",
            assignment.task_id,
            "launch.starting",
            json!({"launch_attempt_id":id,"run_id":run_id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;

        let task = self.get_task(assignment.task_id).await?;
        let context = match self.get_current_context(assignment.task_id).await {
            Ok(value) => Some(value),
            Err(DomainError::NotFound(_)) => None,
            Err(err) => return Err(err),
        };
        Ok(LaunchExecution {
            attempt: self.get_launch_attempt(id).await?,
            run: self.get_run(run_id).await?,
            profile,
            task,
            context,
        })
    }

    pub async fn mark_launch_running(
        &self,
        id: Id,
        pid: Option<i64>,
        stdout_path: String,
        stderr_path: String,
    ) -> Result<LaunchAttempt, DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let attempt = row_to_launch_attempt(
            sqlx::query("SELECT * FROM launch_attempts WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("launch attempt {id}")))?,
        )?;
        if attempt.status != "starting" {
            return Err(DomainError::Conflict(format!(
                "launch attempt is {}",
                attempt.status
            )));
        }
        sqlx::query(
            "UPDATE launch_attempts SET status='running',pid=?,stdout_path=?,stderr_path=? WHERE id=?",
        )
        .bind(pid)
        .bind(&stdout_path)
        .bind(&stderr_path)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "system",
            "launcher",
            "task",
            attempt.task_id,
            "launch.running",
            json!({"launch_attempt_id":id,"pid":pid}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_launch_attempt(id).await
    }

    pub async fn finish_launch_attempt(
        &self,
        id: Id,
        exit_code: Option<i64>,
        external_session_ref: Option<String>,
        error: Option<String>,
    ) -> Result<LaunchAttempt, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let attempt = row_to_launch_attempt(
            sqlx::query("SELECT * FROM launch_attempts WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("launch attempt {id}")))?,
        )?;
        if matches!(
            attempt.status.as_str(),
            "completed" | "failed" | "cancelled"
        ) {
            return Ok(attempt);
        }
        let success = exit_code == Some(0) && error.is_none();
        let terminal_status = if success { "completed" } else { "failed" };
        sqlx::query(
            "UPDATE launch_attempts SET status=?,external_session_ref=COALESCE(?,external_session_ref),exit_code=?,error=?,ended_at=? WHERE id=?",
        )
        .bind(terminal_status)
        .bind(&external_session_ref)
        .bind(exit_code)
        .bind(&error)
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        if let Some(job_id) = attempt.job_id {
            sqlx::query("UPDATE jobs SET status=?,last_error=? WHERE id=?")
                .bind(if success { "done" } else { "failed" })
                .bind(&error)
                .bind(job_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
        }
        if let Some(run_id) = attempt.run_id {
            if external_session_ref.is_some() {
                sqlx::query("UPDATE runs SET external_session_ref=COALESCE(?,external_session_ref) WHERE id=?")
                    .bind(&external_session_ref)
                    .bind(run_id.to_string())
                    .execute(&mut *tx)
                    .await
                    .map_err(storage)?;
            }
            let run_status: Option<String> =
                sqlx::query_scalar("SELECT status FROM runs WHERE id=?")
                    .bind(run_id.to_string())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(storage)?;
            if matches!(run_status.as_deref(), Some("running") | Some("paused")) {
                let new_run_status = if success { "paused" } else { "failed" };
                let stop_reason = if success {
                    "launcher_process_exited_without_completion".to_owned()
                } else {
                    error
                        .clone()
                        .unwrap_or_else(|| format!("launcher_exit_{}", exit_code.unwrap_or(-1)))
                };
                sqlx::query("UPDATE runs SET status=?,stop_reason=?,ended_at=? WHERE id=?")
                    .bind(new_run_status)
                    .bind(&stop_reason)
                    .bind(now.to_rfc3339())
                    .bind(run_id.to_string())
                    .execute(&mut *tx)
                    .await
                    .map_err(storage)?;
                append_event_tx(
                    &mut tx,
                    "system",
                    "launcher",
                    "run",
                    run_id,
                    if success { "run.paused" } else { "run.failed" },
                    json!({"task_id":attempt.task_id,"launch_attempt_id":id,"stop_reason":stop_reason}),
                    None,
                )
                .await?;
                release_launcher_assignment_tx(
                    &mut tx,
                    attempt.assignment_id,
                    attempt.task_id,
                    attempt.agent_instance_id,
                    if success {
                        "launcher_process_exited"
                    } else {
                        "launcher_failed"
                    },
                    now,
                )
                .await?;
            }
        } else if !success {
            release_launcher_assignment_tx(
                &mut tx,
                attempt.assignment_id,
                attempt.task_id,
                attempt.agent_instance_id,
                "launcher_failed_before_run",
                now,
            )
            .await?;
        }
        append_event_tx(
            &mut tx,
            "system",
            "launcher",
            "task",
            attempt.task_id,
            if success {
                "launch.completed"
            } else {
                "launch.failed"
            },
            json!({"launch_attempt_id":id,"exit_code":exit_code,"external_session_ref":external_session_ref,"error":error}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_launch_attempt(id).await
    }
}

async fn release_launcher_assignment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    assignment_id: Id,
    task_id: Id,
    agent_id: Id,
    reason: &str,
    now: DateTime<Utc>,
) -> Result<(), DomainError> {
    let role: Option<String> = sqlx::query_scalar("SELECT role FROM assignments WHERE id=?")
        .bind(assignment_id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?;
    let result = sqlx::query(
        "UPDATE assignments SET status='released',released_at=?,release_reason=? WHERE id=? AND status='active'",
    )
    .bind(now.to_rfc3339())
    .bind(reason)
    .bind(assignment_id.to_string())
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    if result.rows_affected() == 1 {
        append_event_tx(
            tx,
            "system",
            "launcher",
            "task",
            task_id,
            "assignment.released",
            json!({"assignment_id":assignment_id,"agent_instance_id":agent_id,"reason":reason}),
            None,
        )
        .await?;
        if role.as_deref() == Some("executor") {
            sqlx::query(
                "UPDATE tasks SET state='ready',updated_at=? WHERE id=? AND state='in_progress'",
            )
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut **tx)
            .await
            .map_err(storage)?;
            append_event_tx(
                tx,
                "system",
                "launcher",
                "task",
                task_id,
                "task.state_changed",
                json!({"state":"ready","reason":reason}),
                None,
            )
            .await?;
        }
    }
    Ok(())
}

fn validate_absolute(field: &str, value: &str) -> Result<(), DomainError> {
    if value.is_empty() || !Path::new(value).is_absolute() {
        return Err(DomainError::InvalidInput(format!(
            "{field} must be an absolute path"
        )));
    }
    Ok(())
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value.and_then(|v| {
        let trimmed = v.trim().to_owned();
        (!trimmed.is_empty()).then_some(trimmed)
    })
}

fn row_to_launch_profile(row: sqlx::sqlite::SqliteRow) -> Result<LaunchProfile, DomainError> {
    Ok(LaunchProfile {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        name: row.try_get("name").map_err(storage)?,
        adapter: row.try_get("adapter").map_err(storage)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        program: row.try_get("program").map_err(storage)?,
        codex_home: row.try_get("codex_home").map_err(storage)?,
        default_cwd: row.try_get("default_cwd").map_err(storage)?,
        model: row.try_get("model").map_err(storage)?,
        enabled: row.try_get("enabled").map_err(storage)?,
        metadata: parse_json(row.try_get("metadata_json").map_err(storage)?)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}

fn row_to_launch_attempt(row: sqlx::sqlite::SqliteRow) -> Result<LaunchAttempt, DomainError> {
    Ok(LaunchAttempt {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        assignment_id: parse_id(row.try_get("assignment_id").map_err(storage)?)?,
        task_id: parse_id(row.try_get("task_id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        launch_profile_id: parse_id(row.try_get("launch_profile_id").map_err(storage)?)?,
        run_id: parse_opt_id(row.try_get("run_id").map_err(storage)?)?,
        job_id: parse_opt_id(row.try_get("job_id").map_err(storage)?)?,
        status: row.try_get("status").map_err(storage)?,
        cwd: row.try_get("cwd").map_err(storage)?,
        external_session_ref: row.try_get("external_session_ref").map_err(storage)?,
        pid: row.try_get("pid").map_err(storage)?,
        exit_code: row.try_get("exit_code").map_err(storage)?,
        stdout_path: row.try_get("stdout_path").map_err(storage)?,
        stderr_path: row.try_get("stderr_path").map_err(storage)?,
        error: row.try_get("error").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        started_at: parse_opt_dt(row.try_get("started_at").map_err(storage)?)?,
        ended_at: parse_opt_dt(row.try_get("ended_at").map_err(storage)?)?,
    })
}
