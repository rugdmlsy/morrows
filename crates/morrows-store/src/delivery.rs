use super::*;
use morrows_core::AgentDelivery;

pub(crate) async fn enqueue_agent_delivery_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    agent_instance_id: Id,
    task_id: Option<Id>,
    kind: &str,
    source_id: Id,
    payload: Value,
) -> Result<Id, DomainError> {
    let id = Uuid::new_v4();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO agent_deliveries(
            id,agent_instance_id,task_id,kind,source_id,payload_json,status,created_at
         ) VALUES(?,?,?,?,?,?,'queued',?)",
    )
    .bind(id.to_string())
    .bind(agent_instance_id.to_string())
    .bind(task_id.map(|value| value.to_string()))
    .bind(kind)
    .bind(source_id.to_string())
    .bind(payload.to_string())
    .bind(now.to_rfc3339())
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(id)
}

impl Store {
    pub async fn agent_delivery_inbox(
        &self,
        agent_id: Id,
        limit: i64,
    ) -> Result<Vec<AgentDelivery>, DomainError> {
        self.get_agent(agent_id).await?;
        let limit = limit.clamp(1, 200);
        let rows = sqlx::query(
            "SELECT * FROM agent_deliveries
             WHERE agent_instance_id=? AND status='queued'
             ORDER BY created_at,id LIMIT ?",
        )
        .bind(agent_id.to_string())
        .bind(limit)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_agent_delivery).collect()
    }

    pub async fn get_agent_delivery(&self, id: Id) -> Result<AgentDelivery, DomainError> {
        let row = sqlx::query("SELECT * FROM agent_deliveries WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("agent delivery {id}")))?;
        row_to_agent_delivery(row)
    }

    pub async fn acknowledge_agent_delivery(
        &self,
        id: Id,
        agent_id: Id,
        delivered_by: &str,
    ) -> Result<AgentDelivery, DomainError> {
        self.get_agent(agent_id).await?;
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row = sqlx::query("SELECT * FROM agent_deliveries WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("agent delivery {id}")))?;
        let delivery = row_to_agent_delivery(row)?;
        if delivery.agent_instance_id != agent_id {
            return Err(DomainError::Conflict(
                "delivery belongs to another agent instance".into(),
            ));
        }
        if delivery.status == "delivered" {
            tx.commit().await.map_err(storage)?;
            return Ok(delivery);
        }
        if delivery.status == "claimed" {
            return Err(DomainError::Conflict(
                "delivery is currently claimed by a local launch attempt".into(),
            ));
        }
        sqlx::query(
            "UPDATE agent_deliveries
             SET status='delivered',delivered_by=?,delivered_at=?
             WHERE id=? AND status='queued'",
        )
        .bind(delivered_by)
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "agent_delivery",
            id,
            "agent_delivery.delivered",
            json!({"delivered_by": delivered_by}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_agent_delivery(id).await
    }

    pub async fn claim_agent_deliveries_for_launch(
        &self,
        attempt_id: Id,
        limit: i64,
    ) -> Result<Vec<AgentDelivery>, DomainError> {
        let limit = limit.clamp(1, 100);
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let attempt =
            sqlx::query("SELECT status,agent_instance_id,task_id FROM launch_attempts WHERE id=?")
                .bind(attempt_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("launch attempt {attempt_id}")))?;
        let status: String = attempt.try_get("status").map_err(storage)?;
        if status != "running" {
            return Err(DomainError::Conflict(format!("launch attempt is {status}")));
        }
        let agent_instance_id = parse_id(attempt.try_get("agent_instance_id").map_err(storage)?)?;
        let task_id = parse_id(attempt.try_get("task_id").map_err(storage)?)?;

        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM agent_deliveries
             WHERE agent_instance_id=? AND status='queued'
               AND (task_id IS NULL OR task_id=?)
             ORDER BY created_at,id LIMIT ?",
        )
        .bind(agent_instance_id.to_string())
        .bind(task_id.to_string())
        .bind(limit)
        .fetch_all(&mut *tx)
        .await
        .map_err(storage)?;

        for id in &ids {
            sqlx::query(
                "UPDATE agent_deliveries
                 SET status='claimed',claimed_by_launch_attempt_id=?,claimed_at=?
                 WHERE id=? AND status='queued'",
            )
            .bind(attempt_id.to_string())
            .bind(now.to_rfc3339())
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }
        tx.commit().await.map_err(storage)?;

        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let rows = sqlx::query(
            "SELECT * FROM agent_deliveries
             WHERE claimed_by_launch_attempt_id=? AND status='claimed'
             ORDER BY created_at,id",
        )
        .bind(attempt_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_agent_delivery).collect()
    }

    pub async fn complete_claimed_agent_deliveries(
        &self,
        attempt_id: Id,
        delivered_by: &str,
    ) -> Result<u64, DomainError> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE agent_deliveries
             SET status='delivered',delivered_by=?,delivered_at=?
             WHERE claimed_by_launch_attempt_id=? AND status='claimed'",
        )
        .bind(delivered_by)
        .bind(now.to_rfc3339())
        .bind(attempt_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(result.rows_affected())
    }

    pub async fn release_claimed_agent_deliveries(
        &self,
        attempt_id: Id,
    ) -> Result<u64, DomainError> {
        let result = sqlx::query(
            "UPDATE agent_deliveries
             SET status='queued',claimed_by_launch_attempt_id=NULL,claimed_at=NULL
             WHERE claimed_by_launch_attempt_id=? AND status='claimed'",
        )
        .bind(attempt_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(result.rows_affected())
    }

    pub async fn acknowledge_conversation_deliveries(
        &self,
        conversation_id: Id,
        agent_id: Id,
        delivered_by: &str,
    ) -> Result<u64, DomainError> {
        let conversation = self.get_conversation(conversation_id).await?;
        if conversation.agent_instance_id != agent_id {
            return Err(DomainError::Conflict(
                "conversation belongs to another agent instance".into(),
            ));
        }
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE agent_deliveries
             SET status='delivered',delivered_by=?,delivered_at=?
             WHERE agent_instance_id=? AND kind='conversation_message' AND status='queued'
               AND source_id IN (
                 SELECT id FROM conversation_messages WHERE conversation_id=?
               )",
        )
        .bind(delivered_by)
        .bind(now.to_rfc3339())
        .bind(agent_id.to_string())
        .bind(conversation_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(result.rows_affected())
    }

    pub async fn acknowledge_instruction_deliveries_for_task(
        &self,
        task_id: Id,
        agent_id: Id,
        delivered_by: &str,
    ) -> Result<u64, DomainError> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE agent_deliveries
             SET status='delivered',delivered_by=?,delivered_at=?
             WHERE agent_instance_id=? AND task_id=? AND kind='launch_instruction'
               AND status='queued'
               AND source_id IN (
                 SELECT i.id
                 FROM launch_instructions i
                 JOIN launch_attempts a ON a.id=i.launch_attempt_id
                 WHERE a.task_id=? AND a.agent_instance_id=?
               )",
        )
        .bind(delivered_by)
        .bind(now.to_rfc3339())
        .bind(agent_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(result.rows_affected())
    }

    pub async fn recover_agent_delivery_claims(&self) -> Result<u64, DomainError> {
        let result = sqlx::query(
            "UPDATE agent_deliveries
             SET status='queued',claimed_by_launch_attempt_id=NULL,claimed_at=NULL
             WHERE status='claimed'
               AND (
                 claimed_by_launch_attempt_id IS NULL
                 OR NOT EXISTS (
                   SELECT 1 FROM launch_attempts a
                   WHERE a.id=agent_deliveries.claimed_by_launch_attempt_id
                     AND a.status IN ('starting','running')
                 )
               )",
        )
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(result.rows_affected())
    }

    pub async fn delivery_resume_candidates(&self) -> Result<Vec<Id>, DomainError> {
        let now = Utc::now().to_rfc3339();
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT r.id
             FROM runs r
             JOIN assignments ass ON ass.id=r.assignment_id
             JOIN run_lsm_bindings rb ON rb.run_id=r.id
             WHERE r.status='interrupted'
               AND ass.status='active' AND ass.expires_at>?
               AND rb.restart_deadline_at IS NOT NULL AND rb.restart_deadline_at>?
               AND EXISTS (
                 SELECT 1 FROM agent_deliveries d
                 WHERE d.agent_instance_id=r.agent_instance_id
                   AND d.status='queued'
                   AND (d.task_id IS NULL OR d.task_id=r.task_id)
               )
               AND EXISTS (
                 SELECT 1
                 FROM launch_attempts a
                 JOIN launch_profiles p ON p.id=a.launch_profile_id
                 WHERE a.run_id=r.id AND p.adapter IN ('codex_cli','codebuddy_cli')
               )
               AND NOT EXISTS (
                 SELECT 1 FROM launch_attempts active
                 WHERE active.assignment_id=r.assignment_id
                   AND active.status IN ('queued','starting','running')
               )
             ORDER BY r.started_at,r.id
             LIMIT 32",
        )
        .bind(&now)
        .bind(&now)
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(parse_id).collect()
    }
}

fn row_to_agent_delivery(row: sqlx::sqlite::SqliteRow) -> Result<AgentDelivery, DomainError> {
    Ok(AgentDelivery {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        task_id: parse_opt_id(row.try_get("task_id").map_err(storage)?)?,
        kind: row.try_get("kind").map_err(storage)?,
        source_id: parse_id(row.try_get("source_id").map_err(storage)?)?,
        payload: parse_json(row.try_get("payload_json").map_err(storage)?)?,
        status: row.try_get("status").map_err(storage)?,
        claimed_by_launch_attempt_id: parse_opt_id(
            row.try_get("claimed_by_launch_attempt_id")
                .map_err(storage)?,
        )?,
        claimed_at: parse_opt_dt(row.try_get("claimed_at").map_err(storage)?)?,
        delivered_by: row.try_get("delivered_by").map_err(storage)?,
        delivered_at: parse_opt_dt(row.try_get("delivered_at").map_err(storage)?)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}
