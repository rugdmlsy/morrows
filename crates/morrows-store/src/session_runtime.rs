use super::*;
use morrows_core::{SessionRuntimeAttempt, StartSessionRuntime};
use std::path::Path;

impl Store {
    pub async fn recover_session_runtime_attempts_after_restart(&self) -> Result<u64, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let session_ids: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT session_id FROM session_runtime_attempts
             WHERE status IN ('queued','running')",
        )
        .fetch_all(&mut *tx)
        .await
        .map_err(storage)?;

        let result = sqlx::query(
            "UPDATE session_runtime_attempts
             SET status='failed',
                 error=COALESCE(error,'morrows_server_restarted'),
                 ended_at=COALESCE(ended_at,?)
             WHERE status IN ('queued','running')",
        )
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        for session_id in session_ids {
            sqlx::query(
                "UPDATE agent_credentials
                 SET revoked_at=COALESCE(revoked_at,?)
                 WHERE kind='session_runtime' AND session_id=? AND revoked_at IS NULL",
            )
            .bind(now.to_rfc3339())
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }
        tx.commit().await.map_err(storage)?;
        Ok(result.rows_affected())
    }

    pub async fn create_session_runtime_attempt(
        &self,
        session_id: Id,
        input: StartSessionRuntime,
    ) -> Result<SessionRuntimeAttempt, DomainError> {
        let session = self.get_session(session_id).await?;
        if session.status != "open" {
            return Err(DomainError::Conflict("session is archived".into()));
        }
        let agent = self.get_agent(session.agent_instance_id).await?;
        if agent.archived_at.is_some() {
            return Err(DomainError::Conflict("agent is archived".into()));
        }

        let profiles = self
            .list_launch_profiles()
            .await?
            .into_iter()
            .filter(|profile| {
                profile.agent_instance_id == agent.id
                    && profile.enabled
                    && matches!(profile.adapter.as_str(), "codex_cli" | "codebuddy_cli")
            })
            .collect::<Vec<_>>();

        let profile = if let Some(profile_id) = input.launch_profile_id {
            let profile = profiles
                .into_iter()
                .find(|profile| profile.id == profile_id)
                .ok_or_else(|| {
                    DomainError::Conflict(
                        "requested launch profile is not an enabled local CLI profile for this Agent"
                            .into(),
                    )
                })?;
            profile
        } else {
            match profiles.as_slice() {
                [profile] => profile.clone(),
                [] => {
                    return Err(DomainError::Conflict(
                        "this Agent has no enabled local CLI launch profile".into(),
                    ));
                }
                _ => {
                    return Err(DomainError::Conflict(
                        "this Agent has multiple local CLI launch profiles; specify launch_profile_id"
                            .into(),
                    ));
                }
            }
        };

        if !Path::new(&profile.program).is_file() {
            return Err(DomainError::InvalidInput(format!(
                "launch program does not exist or is not a file: {}",
                profile.program
            )));
        }
        let cwd = profile.default_cwd.clone().ok_or_else(|| {
            DomainError::InvalidInput("session CLI launch profile must define default_cwd".into())
        })?;
        if !Path::new(&cwd).is_dir() {
            return Err(DomainError::InvalidInput(format!(
                "launch cwd does not exist or is not a directory: {cwd}"
            )));
        }
        if let Some(account_id) = agent.account_id {
            self.get_account(account_id).await?;
        }

        let previous = self.latest_session_runtime_attempt(session_id).await?;
        let model = resolve_runtime_model(
            input.model.as_deref(),
            previous
                .as_ref()
                .and_then(|attempt| attempt.model.as_deref()),
            profile.model.as_deref(),
        )?;
        let reasoning_effort = resolve_reasoning_effort(
            input.reasoning_effort.as_deref(),
            previous
                .as_ref()
                .and_then(|attempt| attempt.reasoning_effort.as_deref()),
        )?;

        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let active: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM session_runtime_attempts
             WHERE session_id=? AND status IN ('queued','running')",
        )
        .bind(session_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if active > 0 {
            return Err(DomainError::Conflict(
                "Session already has an active Agent runtime".into(),
            ));
        }

        sqlx::query(
            "INSERT INTO session_runtime_attempts(
                id,session_id,agent_instance_id,account_id,launch_profile_id,
                adapter,status,model,reasoning_effort,cwd,created_at
             ) VALUES(?,?,?,?,?,?,'queued',?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(session_id.to_string())
        .bind(agent.id.to_string())
        .bind(agent.account_id.map(|value| value.to_string()))
        .bind(profile.id.to_string())
        .bind(&profile.adapter)
        .bind(model.as_deref())
        .bind(reasoning_effort.as_deref())
        .bind(&cwd)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        append_event_tx(
            &mut tx,
            "operator",
            "control-plane",
            "session",
            session_id,
            "session.runtime_queued",
            json!({
                "runtime_attempt_id": id,
                "agent_instance_id": agent.id,
                "account_id": agent.account_id,
                "launch_profile_id": profile.id,
                "adapter": profile.adapter,
                "model": model,
                "reasoning_effort": reasoning_effort,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_session_runtime_attempt(id).await
    }

    pub async fn get_session_runtime_attempt(
        &self,
        id: Id,
    ) -> Result<SessionRuntimeAttempt, DomainError> {
        let row = sqlx::query("SELECT * FROM session_runtime_attempts WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("session runtime attempt {id}")))?;
        row_to_session_runtime_attempt(row)
    }

    pub async fn latest_session_runtime_attempt(
        &self,
        session_id: Id,
    ) -> Result<Option<SessionRuntimeAttempt>, DomainError> {
        self.get_session(session_id).await?;
        let row = sqlx::query(
            "SELECT * FROM session_runtime_attempts
             WHERE session_id=? ORDER BY created_at DESC,id DESC LIMIT 1",
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        row.map(row_to_session_runtime_attempt).transpose()
    }

    pub async fn latest_session_provider_ref(
        &self,
        session_id: Id,
        launch_profile_id: Id,
    ) -> Result<Option<String>, DomainError> {
        let value: Option<String> = sqlx::query_scalar(
            "SELECT provider_session_ref
             FROM session_runtime_attempts
             WHERE session_id=? AND launch_profile_id=?
               AND provider_session_ref IS NOT NULL
             ORDER BY created_at DESC,id DESC LIMIT 1",
        )
        .bind(session_id.to_string())
        .bind(launch_profile_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .flatten();
        Ok(value)
    }

    pub async fn mark_session_runtime_running(
        &self,
        id: Id,
        pid: Option<i64>,
        stdout_path: String,
        stderr_path: String,
    ) -> Result<SessionRuntimeAttempt, DomainError> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE session_runtime_attempts
             SET status='running',pid=?,stdout_path=?,stderr_path=?,started_at=?
             WHERE id=? AND status='queued'",
        )
        .bind(pid)
        .bind(stdout_path)
        .bind(stderr_path)
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        if result.rows_affected() != 1 {
            return Err(DomainError::Conflict(
                "session runtime attempt is no longer queued".into(),
            ));
        }
        self.get_session_runtime_attempt(id).await
    }

    pub async fn finish_session_runtime_attempt(
        &self,
        id: Id,
        exit_code: Option<i64>,
        provider_session_ref: Option<String>,
        error: Option<String>,
    ) -> Result<SessionRuntimeAttempt, DomainError> {
        let current = self.get_session_runtime_attempt(id).await?;
        if matches!(current.status.as_str(), "completed" | "failed") {
            return Ok(current);
        }
        let now = Utc::now();
        let status = if error.is_none() && exit_code == Some(0) {
            "completed"
        } else {
            "failed"
        };
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "UPDATE session_runtime_attempts
             SET status=?,exit_code=?,provider_session_ref=COALESCE(?,provider_session_ref),
                 error=?,ended_at=?
             WHERE id=?",
        )
        .bind(status)
        .bind(exit_code)
        .bind(provider_session_ref.as_deref())
        .bind(error.as_deref())
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        append_event_tx(
            &mut tx,
            "system",
            "session-runtime",
            "session",
            current.session_id,
            if status == "completed" {
                "session.runtime_completed"
            } else {
                "session.runtime_failed"
            },
            json!({
                "runtime_attempt_id": id,
                "exit_code": exit_code,
                "provider_session_ref": provider_session_ref,
                "error": error,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_session_runtime_attempt(id).await
    }
}

fn resolve_runtime_model(
    requested: Option<&str>,
    previous: Option<&str>,
    profile_default: Option<&str>,
) -> Result<Option<String>, DomainError> {
    let chosen = match requested {
        Some(value) if value.trim().is_empty() => profile_default,
        Some(value) => Some(value),
        None => previous.or(profile_default),
    };
    let Some(value) = chosen.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > 128 {
        return Err(DomainError::InvalidInput(
            "Session runtime model must be at most 128 characters".into(),
        ));
    }
    Ok(Some(value.to_owned()))
}

fn resolve_reasoning_effort(
    requested: Option<&str>,
    previous: Option<&str>,
) -> Result<Option<String>, DomainError> {
    let chosen = match requested {
        Some(value) if value.trim().is_empty() => None,
        Some(value) => Some(value),
        None => previous,
    };
    let Some(value) = chosen.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value.chars().count() > 32
        || !value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return Err(DomainError::InvalidInput(
            "invalid Session runtime reasoning_effort".into(),
        ));
    }
    Ok(Some(value.to_owned()))
}

fn row_to_session_runtime_attempt(
    row: sqlx::sqlite::SqliteRow,
) -> Result<SessionRuntimeAttempt, DomainError> {
    Ok(SessionRuntimeAttempt {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        session_id: parse_id(row.try_get("session_id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        account_id: parse_opt_id(row.try_get("account_id").map_err(storage)?)?,
        launch_profile_id: parse_id(row.try_get("launch_profile_id").map_err(storage)?)?,
        adapter: row.try_get("adapter").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        model: row.try_get("model").map_err(storage)?,
        reasoning_effort: row.try_get("reasoning_effort").map_err(storage)?,
        cwd: row.try_get("cwd").map_err(storage)?,
        provider_session_ref: row.try_get("provider_session_ref").map_err(storage)?,
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
