use super::*;
use crate::delivery::enqueue_agent_delivery_tx;
use morrows_core::{
    INTERVIEW_STATE_CONVERGED, INTERVIEW_STATE_NOT_STARTED, INTERVIEW_STATE_WAITING_FOR_AGENT,
    INTERVIEW_STATE_WAITING_FOR_HUMAN, Message, MessageThread,
};

impl Store {
    pub(crate) async fn ensure_task_agent_thread_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        task_id: Id,
        agent_id: Id,
    ) -> Result<Id, DomainError> {
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM message_threads
             WHERE task_id=? AND kind='human_agent' AND target_agent_instance_id=?
             ORDER BY created_at DESC,id DESC LIMIT 1",
        )
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?
        {
            return parse_id(existing);
        }

        let task_title: String = sqlx::query_scalar("SELECT title FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?;
        let agent_name: String =
            sqlx::query_scalar("SELECT display_name FROM agent_instances WHERE id=?")
                .bind(agent_id.to_string())
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("agent {agent_id}")))?;

        let id = Uuid::new_v4();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO message_threads(
                id,task_id,created_by,title,created_at,kind,target_agent_instance_id
             ) VALUES(?,?,?,?,?,'human_agent',?)",
        )
        .bind(id.to_string())
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .bind(format!("{task_title} · {agent_name}"))
        .bind(now.to_rfc3339())
        .bind(agent_id.to_string())
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            tx,
            "system",
            "task-messaging",
            "task",
            task_id,
            "thread.created",
            json!({"thread_id":id,"kind":"human_agent","target_agent_instance_id":agent_id}),
            None,
        )
        .await?;
        Ok(id)
    }

    pub async fn task_agent_thread(
        &self,
        task_id: Id,
        agent_id: Id,
    ) -> Result<Option<MessageThread>, DomainError> {
        self.get_task(task_id).await?;
        self.get_agent(agent_id).await?;
        let row = sqlx::query(
            "SELECT * FROM message_threads
             WHERE task_id=? AND kind='human_agent' AND target_agent_instance_id=?
             ORDER BY created_at DESC,id DESC LIMIT 1",
        )
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        row.map(crate::collaboration::row_to_thread).transpose()
    }

    pub async fn create_human_task_message(
        &self,
        task_id: Id,
        agent_id: Id,
        body: &str,
        client_message_id: Option<&str>,
    ) -> Result<Message, DomainError> {
        let body = body.trim();
        if body.is_empty() {
            return Err(DomainError::InvalidInput(
                "message body cannot be empty".into(),
            ));
        }
        if body.chars().count() > 100_000 {
            return Err(DomainError::InvalidInput(
                "message body must be at most 100000 characters".into(),
            ));
        }
        self.get_task(task_id).await?;
        self.get_agent(agent_id).await?;
        let client_message_id = client_message_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        if client_message_id
            .as_ref()
            .is_some_and(|value| value.chars().count() > 128)
        {
            return Err(DomainError::InvalidInput(
                "client_message_id must be at most 128 characters".into(),
            ));
        }

        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let thread_id = self
            .ensure_task_agent_thread_tx(&mut tx, task_id, agent_id)
            .await?;

        if let Some(client_id) = client_message_id.as_deref() {
            if let Some(row) = sqlx::query(
                "SELECT * FROM messages WHERE thread_id=? AND client_message_id=? LIMIT 1",
            )
            .bind(thread_id.to_string())
            .bind(client_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            {
                let existing = crate::collaboration::row_to_message(row)?;
                if existing.body != body {
                    return Err(DomainError::Conflict(
                        "client_message_id was already used for different content".into(),
                    ));
                }
                tx.commit().await.map_err(storage)?;
                return Ok(existing);
            }
        }

        let id = Uuid::new_v4();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO messages(
                id,thread_id,created_by,author_type,body,created_at,
                message_type,recipient_agent_instance_id,requires_response,status,
                client_message_id,recalled_at
             ) VALUES(?,?,NULL,'human',?,?,'human_agent',?,1,'queued',?,NULL)",
        )
        .bind(id.to_string())
        .bind(thread_id.to_string())
        .bind(body)
        .bind(now.to_rfc3339())
        .bind(agent_id.to_string())
        .bind(client_message_id.as_deref())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        let delivery_id = enqueue_agent_delivery_tx(
            &mut tx,
            agent_id,
            Some(task_id),
            "task_message",
            id,
            json!({"thread_id":thread_id,"message_id":id,"body":body}),
        )
        .await?;

        sqlx::query(
            "UPDATE assignment_intakes
             SET conversation_state=?,interview_status='pending',human_response=NULL,
                 approved_by_actor_id=NULL,approved_at=NULL,final_summary_message_id=NULL,
                 confirmation_message_id=NULL,converged_at=NULL,updated_at=?
             WHERE interview_thread_id=? AND conversation_state!=?
               AND assignment_id IN (
                 SELECT id FROM assignments
                 WHERE status='active' AND phase IN ('human_interview','ready')
               )",
        )
        .bind(INTERVIEW_STATE_WAITING_FOR_AGENT)
        .bind(now.to_rfc3339())
        .bind(thread_id.to_string())
        .bind(INTERVIEW_STATE_NOT_STARTED)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE assignments SET phase='human_interview',expires_at=MAX(expires_at,?),renewed_at=?
             WHERE id IN (
               SELECT assignment_id FROM assignment_intakes WHERE interview_thread_id=?
             ) AND status='active' AND phase IN ('human_interview','ready')",
        )
        .bind((now + Duration::hours(24)).to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(thread_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        append_event_tx(
            &mut tx,
            "human",
            "local",
            "task",
            task_id,
            "task.message_queued",
            json!({
                "thread_id":thread_id,
                "message_id":id,
                "agent_instance_id":agent_id,
                "delivery_id":delivery_id,
                "client_message_id":client_message_id,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_task_message(id).await
    }

    pub async fn get_task_message(&self, id: Id) -> Result<Message, DomainError> {
        let row = sqlx::query("SELECT * FROM messages WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task message {id}")))?;
        crate::collaboration::row_to_message(row)
    }

    pub async fn recall_human_task_message(
        &self,
        task_id: Id,
        agent_id: Id,
        message_id: Id,
    ) -> Result<Message, DomainError> {
        self.get_task(task_id).await?;
        self.get_agent(agent_id).await?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;

        let row = sqlx::query(
            "SELECT m.*
             FROM messages m
             JOIN message_threads t ON t.id=m.thread_id
             WHERE m.id=? AND t.task_id=? AND t.kind='human_agent'
               AND t.target_agent_instance_id=?",
        )
        .bind(message_id.to_string())
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("task message {message_id}")))?;
        let existing = crate::collaboration::row_to_message(row)?;
        if existing.author_type != "human" {
            return Err(DomainError::Conflict(
                "only Human task messages can be recalled".into(),
            ));
        }
        if existing.recalled_at.is_some() {
            tx.commit().await.map_err(storage)?;
            return Ok(existing);
        }

        let delivery_status: Option<String> = sqlx::query_scalar(
            "SELECT status FROM agent_deliveries
             WHERE kind='task_message' AND source_id=?",
        )
        .bind(message_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?;
        if delivery_status.as_deref() != Some("queued") {
            return Err(DomainError::Conflict(
                "task message can only be recalled before delivery".into(),
            ));
        }

        let now = Utc::now();
        sqlx::query(
            "UPDATE messages
             SET recalled_at=?,status='recalled'
             WHERE id=? AND recalled_at IS NULL",
        )
        .bind(now.to_rfc3339())
        .bind(message_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "DELETE FROM agent_deliveries
             WHERE kind='task_message' AND source_id=? AND status='queued'",
        )
        .bind(message_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "human",
            "local",
            "task",
            task_id,
            "task.message_recalled",
            json!({"message_id":message_id,"agent_instance_id":agent_id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_task_message(message_id).await
    }

    pub async fn agent_reply_task_thread(
        &self,
        thread_id: Id,
        agent_id: Id,
        body: &str,
    ) -> Result<Message, DomainError> {
        let body = body.trim();
        if body.is_empty() {
            return Err(DomainError::InvalidInput(
                "message body cannot be empty".into(),
            ));
        }
        self.get_agent(agent_id).await?;

        let thread = sqlx::query(
            "SELECT task_id,kind,target_agent_instance_id FROM message_threads WHERE id=?",
        )
        .bind(thread_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("thread {thread_id}")))?;
        let task_id = parse_id(thread.try_get::<String, _>("task_id").map_err(storage)?)?;
        let kind: String = thread.try_get("kind").map_err(storage)?;
        let target: Option<String> = thread
            .try_get("target_agent_instance_id")
            .map_err(storage)?;
        if kind != "human_agent" || target.as_deref() != Some(agent_id.to_string().as_str()) {
            return Err(DomainError::Conflict(
                "thread is not the human/agent thread for this AgentInstance".into(),
            ));
        }

        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        sqlx::query(
            "UPDATE agent_deliveries
             SET status='delivered',delivered_by=?,delivered_at=?
             WHERE agent_instance_id=? AND task_id=? AND kind='task_message' AND status='queued'
               AND source_id IN (
                 SELECT id FROM messages
                 WHERE thread_id=? AND author_type='human' AND status='queued' AND recalled_at IS NULL
               )",
        )
        .bind(format!("message_create:{agent_id}"))
        .bind(now.to_rfc3339())
        .bind(agent_id.to_string())
        .bind(task_id.to_string())
        .bind(thread_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE messages SET status='delivered'
             WHERE thread_id=? AND author_type='human' AND status='queued' AND recalled_at IS NULL",
        )
        .bind(thread_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "INSERT INTO messages(
                id,thread_id,created_by,author_type,body,created_at,
                message_type,requires_response,status
             ) VALUES(?,?,?,'agent',?,?,'human_agent',0,'delivered')",
        )
        .bind(id.to_string())
        .bind(thread_id.to_string())
        .bind(agent_id.to_string())
        .bind(body)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        sqlx::query(
            "UPDATE assignment_intakes
             SET conversation_state=?,interview_status='pending',updated_at=?
             WHERE interview_thread_id=? AND conversation_state!=?
               AND assignment_id IN (
                 SELECT id FROM assignments WHERE status='active' AND phase='human_interview'
               )",
        )
        .bind(INTERVIEW_STATE_WAITING_FOR_HUMAN)
        .bind(now.to_rfc3339())
        .bind(thread_id.to_string())
        .bind(INTERVIEW_STATE_CONVERGED)
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE assignments SET expires_at=MAX(expires_at,?),renewed_at=?
             WHERE id IN (
               SELECT assignment_id FROM assignment_intakes WHERE interview_thread_id=?
             ) AND status='active' AND phase='human_interview'",
        )
        .bind((now + Duration::hours(24)).to_rfc3339())
        .bind(now.to_rfc3339())
        .bind(thread_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task",
            task_id,
            "task.message_replied",
            json!({"thread_id":thread_id,"message_id":id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_task_message(id).await
    }

    pub async fn acknowledge_task_thread_deliveries(
        &self,
        thread_id: Id,
        agent_id: Id,
        delivered_by: &str,
    ) -> Result<u64, DomainError> {
        let target: Option<String> = sqlx::query_scalar(
            "SELECT target_agent_instance_id FROM message_threads WHERE id=? AND kind='human_agent'",
        )
        .bind(thread_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .flatten();
        if target.as_deref() != Some(agent_id.to_string().as_str()) {
            return Err(DomainError::Conflict(
                "thread belongs to another agent instance".into(),
            ));
        }
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE agent_deliveries
             SET status='delivered',delivered_by=?,delivered_at=?
             WHERE agent_instance_id=? AND kind='task_message' AND status='queued'
               AND source_id IN (
                 SELECT id FROM messages WHERE thread_id=? AND recalled_at IS NULL
               )",
        )
        .bind(delivered_by)
        .bind(now.to_rfc3339())
        .bind(agent_id.to_string())
        .bind(thread_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(result.rows_affected())
    }
}
