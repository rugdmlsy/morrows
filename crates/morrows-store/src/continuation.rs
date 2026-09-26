use super::*;
use morrows_core::TaskContinuationPolicy;

impl Store {
    pub async fn set_task_continuation_policy(
        &self,
        task: Id,
        policy: TaskContinuationPolicy,
    ) -> Result<TaskContinuationPolicy, DomainError> {
        self.get_task(task).await?;
        let distinct: std::collections::HashSet<_> = policy.agent_ids.iter().collect();
        if policy.agent_ids.len() > 32
            || distinct.len() != policy.agent_ids.len()
            || (policy.enabled && policy.agent_ids.is_empty())
        {
            return Err(DomainError::InvalidInput(
                "continuation needs 1..32 distinct Agent IDs when enabled".into(),
            ));
        }
        for id in &policy.agent_ids {
            self.get_agent(*id).await?;
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        sqlx::query("INSERT INTO task_continuation_policies(task_id,enabled,agent_ids_json,updated_at) VALUES(?,?,?,?) ON CONFLICT(task_id) DO UPDATE SET enabled=excluded.enabled,agent_ids_json=excluded.agent_ids_json,updated_at=excluded.updated_at")
            .bind(task.to_string()).bind(policy.enabled).bind(json!(policy.agent_ids).to_string()).bind(Utc::now().to_rfc3339())
            .execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(
            &mut tx,
            "operator",
            "control-plane",
            "task",
            task,
            "continuation.policy_updated",
            json!(policy),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(policy)
    }

    pub async fn task_continuation_policy(
        &self,
        task: Id,
    ) -> Result<Option<TaskContinuationPolicy>, DomainError> {
        self.get_task(task).await?;
        let mut conn = self.pool.acquire().await.map_err(storage)?;
        load_policy(&mut conn, task).await
    }

    /// Only the current executor receives predecessor checkpoints. General task
    /// discovery keeps provider Sessions and other Agents' run-local data private.
    /// A checkpoint is a last-known snapshot, never proof of clean shutdown or of
    /// the current filesystem; successors must recheck it before repeating effects.
    pub async fn task_recovery_context(&self, task: Id, actor: Id) -> Result<Value, DomainError> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let authorized: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assignments WHERE task_id=? AND agent_instance_id=? AND role='executor' AND status='active' AND expires_at>?)")
            .bind(task.to_string()).bind(actor.to_string()).bind(Utc::now().to_rfc3339())
            .fetch_one(&mut *tx).await.map_err(storage)?;
        if !authorized {
            return Ok(json!({"available":false,"reason":"current_executor_required"}));
        }
        let row = sqlx::query("SELECT r.id,r.agent_instance_id,r.status,COALESCE(r.stop_reason,a.release_reason) AS stop_reason,r.checkpoint_json,r.started_at,r.ended_at,COALESCE(h.context_revision_id,c.context_revision_id) AS context_revision_id,h.id AS handoff_id,
            (SELECT MAX(e.created_at) FROM events e WHERE e.entity_type='run' AND e.entity_id=r.id AND e.event_type='run.checkpointed') AS checkpoint_at
            FROM runs r JOIN assignments a ON a.id=r.assignment_id LEFT JOIN run_context_revisions c ON c.run_id=r.id LEFT JOIN handoffs h ON h.source_run_id=r.id
            WHERE r.task_id=? AND a.role='executor' AND a.status!='active' ORDER BY r.started_at DESC,r.rowid DESC LIMIT 1")
            .bind(task.to_string()).fetch_optional(&mut *tx).await.map_err(storage)?;
        let Some(row) = row else {
            return Ok(json!({"available":false,"reason":"no_predecessor_run"}));
        };
        Ok(
            json!({"available":true,"source_run_id":row.try_get::<String,_>("id").map_err(storage)?,
            "source_agent_id":row.try_get::<String,_>("agent_instance_id").map_err(storage)?,
            "status":row.try_get::<String,_>("status").map_err(storage)?,
            "stop_reason":row.try_get::<Option<String>,_>("stop_reason").map_err(storage)?,
            "checkpoint":parse_opt_json(row.try_get("checkpoint_json").map_err(storage)?)?,
            "checkpoint_at":row.try_get::<Option<String>,_>("checkpoint_at").map_err(storage)?,
            "context_revision_id":row.try_get::<Option<String>,_>("context_revision_id").map_err(storage)?,
            "handoff_id":row.try_get::<Option<String>,_>("handoff_id").map_err(storage)?,
            "started_at":row.try_get::<String,_>("started_at").map_err(storage)?,
            "ended_at":row.try_get::<Option<String>,_>("ended_at").map_err(storage)?,
            "freshness":"last_persisted_only; uncheckpointed work and external effects may exist; verify before retrying"}),
        )
    }
}

pub(crate) async fn load_policy(
    conn: &mut sqlx::SqliteConnection,
    task: Id,
) -> Result<Option<TaskContinuationPolicy>, DomainError> {
    sqlx::query("SELECT enabled,agent_ids_json FROM task_continuation_policies WHERE task_id=?")
        .bind(task.to_string())
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage)?
        .map(|row| {
            Ok(TaskContinuationPolicy {
                enabled: row.try_get("enabled").map_err(storage)?,
                agent_ids: serde_json::from_str(
                    &row.try_get::<String, _>("agent_ids_json")
                        .map_err(storage)?,
                )
                .map_err(storage)?,
            })
        })
        .transpose()
}

/// Releasing a lease is not proof that the old process stopped. Keep the task
/// fenced until managed children exit and LSM terminalization is acknowledged.
pub(crate) async fn predecessor_runtime_active(
    conn: &mut sqlx::SqliteConnection,
    task: Id,
) -> Result<bool, DomainError> {
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM launch_attempts l JOIN launch_profiles p ON p.id=l.launch_profile_id
        WHERE l.task_id=? AND p.adapter IN ('codex_cli','codebuddy_cli') AND l.status IN ('starting','running'))
        OR EXISTS(SELECT 1 FROM runs r LEFT JOIN run_lsm_bindings b ON b.run_id=r.id LEFT JOIN run_lsm_provisioning p ON p.run_id=r.id
            WHERE r.task_id=? AND r.status IN ('handed_off','completed') AND (b.run_id IS NOT NULL OR p.run_id IS NOT NULL) AND b.session_terminalized_at IS NULL)")
        .bind(task.to_string()).bind(task.to_string()).fetch_one(&mut *conn).await.map_err(storage)
}
