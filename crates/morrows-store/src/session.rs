use super::*;
use crate::delivery::enqueue_agent_delivery_tx;
use morrows_core::{
    CreateSession, CreateSessionSummaryRevision, Session, SessionHistory, SessionMessage,
    SessionSummary, SessionSummaryRevision, UpdateSessionScope,
};

impl Store {
    pub async fn active_launch_attempts_for_agent(
        &self,
        agent_id: Id,
    ) -> Result<Vec<Id>, DomainError> {
        self.get_agent(agent_id).await?;
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT id FROM launch_attempts
             WHERE agent_instance_id=? AND status IN ('starting','running')
             ORDER BY created_at DESC,id DESC",
        )
        .bind(agent_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(parse_id).collect()
    }

    pub async fn create_session(&self, input: CreateSession) -> Result<Session, DomainError> {
        self.create_scoped_session(input, None, None).await
    }

    pub async fn create_scoped_session(
        &self,
        input: CreateSession,
        project_id: Option<Id>,
        task_id: Option<Id>,
    ) -> Result<Session, DomainError> {
        let agent = self.get_agent(input.agent_instance_id).await?;
        let (project_id, task_id) = self.resolve_session_scope(project_id, task_id).await?;
        let title = if input.title.trim().is_empty() {
            format!("Session with {}", agent.display_name)
        } else {
            input.title.trim().to_owned()
        };
        if title.chars().count() > 200 {
            return Err(DomainError::InvalidInput(
                "session title must be at most 200 characters".into(),
            ));
        }
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO sessions(
                id,agent_instance_id,project_id,task_id,title,status,created_at,updated_at
             ) VALUES(?,?,?,?,?,'open',?,?)",
        )
        .bind(id.to_string())
        .bind(input.agent_instance_id.to_string())
        .bind(project_id.map(|value| value.to_string()))
        .bind(task_id.map(|value| value.to_string()))
        .bind(&title)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "human",
            "local",
            "session",
            id,
            "session.created",
            json!({
                "agent_instance_id": input.agent_instance_id,
                "project_id": project_id,
                "task_id": task_id,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_session(id).await
    }

    pub async fn update_session_scope(
        &self,
        id: Id,
        input: UpdateSessionScope,
    ) -> Result<Session, DomainError> {
        self.get_session(id).await?;
        let (project_id, task_id) = self
            .resolve_session_scope(input.project_id, input.task_id)
            .await?;
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        sqlx::query("UPDATE sessions SET project_id=?,task_id=?,updated_at=? WHERE id=?")
            .bind(project_id.map(|value| value.to_string()))
            .bind(task_id.map(|value| value.to_string()))
            .bind(now.to_rfc3339())
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        sqlx::query(
            "UPDATE agent_deliveries
             SET task_id=?
             WHERE kind='session_message' AND status='queued'
               AND source_id IN (SELECT id FROM session_messages WHERE session_id=?)",
        )
        .bind(task_id.map(|value| value.to_string()))
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "human",
            "local",
            "session",
            id,
            "session.scope_updated",
            json!({"project_id": project_id, "task_id": task_id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_session(id).await
    }

    async fn resolve_session_scope(
        &self,
        project_id: Option<Id>,
        task_id: Option<Id>,
    ) -> Result<(Option<Id>, Option<Id>), DomainError> {
        if let Some(task_id) = task_id {
            let task = self.get_task(task_id).await?;
            if project_id.is_some() && project_id != task.project_id {
                return Err(DomainError::Conflict(
                    "session project does not match the selected task".into(),
                ));
            }
            return Ok((task.project_id, Some(task_id)));
        }
        if let Some(project_id) = project_id {
            self.get_project(project_id).await?;
            return Ok((Some(project_id), None));
        }
        Ok((None, None))
    }

    pub(crate) async fn ensure_task_session_for_agent_tx(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        task_id: Id,
        agent_id: Id,
    ) -> Result<Id, DomainError> {
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT id FROM sessions
             WHERE task_id=? AND agent_instance_id=? AND status='open'
             ORDER BY updated_at DESC,id DESC LIMIT 1",
        )
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?
        {
            return parse_id(existing);
        }

        let task = sqlx::query("SELECT title,project_id FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?;
        let task_title: String = task.try_get("title").map_err(storage)?;
        let project_id: Option<String> = task.try_get("project_id").map_err(storage)?;
        let display_name: String =
            sqlx::query_scalar("SELECT display_name FROM agent_instances WHERE id=?")
                .bind(agent_id.to_string())
                .fetch_optional(&mut **tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("agent {agent_id}")))?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let title = format!("{task_title} · {display_name}");
        sqlx::query(
            "INSERT INTO sessions(
                id,agent_instance_id,project_id,task_id,title,status,created_at,updated_at
             ) VALUES(?,?,?,?,?,'open',?,?)",
        )
        .bind(id.to_string())
        .bind(agent_id.to_string())
        .bind(project_id)
        .bind(task_id.to_string())
        .bind(title)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            tx,
            "system",
            "launcher",
            "session",
            id,
            "session.created_for_task",
            json!({"agent_instance_id": agent_id, "task_id": task_id}),
            None,
        )
        .await?;
        Ok(id)
    }

    pub async fn get_session(&self, id: Id) -> Result<Session, DomainError> {
        let row = sqlx::query("SELECT * FROM sessions WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("session {id}")))?;
        row_to_session(row)
    }

    pub async fn list_sessions(
        &self,
        agent_id: Option<Id>,
    ) -> Result<Vec<SessionSummary>, DomainError> {
        if let Some(id) = agent_id {
            self.get_agent(id).await?;
        }
        let rows = if let Some(id) = agent_id {
            sqlx::query(
                "SELECT c.*,a.display_name AS agent_name,p.name AS project_name,t.title AS task_title,
                  (SELECT COUNT(*) FROM session_messages m WHERE m.session_id=c.id AND m.recalled_at IS NULL) AS message_count,
                  (SELECT COUNT(*) FROM session_messages m WHERE m.session_id=c.id AND m.author_type='human' AND m.status='queued' AND m.recalled_at IS NULL) AS queued_count,
                  (SELECT COUNT(*) FROM agent_deliveries d
                     JOIN session_messages dm ON dm.id=d.source_id
                     WHERE d.kind='session_message' AND dm.session_id=c.id
                       AND d.status IN ('queued','claimed')) AS undelivered_count,
                  (SELECT substr(m.body,1,160) FROM session_messages m WHERE m.session_id=c.id AND m.recalled_at IS NULL ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_preview,
                  (SELECT m.author_type FROM session_messages m WHERE m.session_id=c.id AND m.recalled_at IS NULL ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_author_type,
                  (SELECT m.created_at FROM session_messages m WHERE m.session_id=c.id AND m.recalled_at IS NULL ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_at
                 FROM sessions c JOIN agent_instances a ON a.id=c.agent_instance_id
                 LEFT JOIN projects p ON p.id=c.project_id
                 LEFT JOIN tasks t ON t.id=c.task_id
                 WHERE c.agent_instance_id=? ORDER BY c.updated_at DESC,c.id DESC",
            )
            .bind(id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
        } else {
            sqlx::query(
                "SELECT c.*,a.display_name AS agent_name,p.name AS project_name,t.title AS task_title,
                  (SELECT COUNT(*) FROM session_messages m WHERE m.session_id=c.id AND m.recalled_at IS NULL) AS message_count,
                  (SELECT COUNT(*) FROM session_messages m WHERE m.session_id=c.id AND m.author_type='human' AND m.status='queued' AND m.recalled_at IS NULL) AS queued_count,
                  (SELECT COUNT(*) FROM agent_deliveries d
                     JOIN session_messages dm ON dm.id=d.source_id
                     WHERE d.kind='session_message' AND dm.session_id=c.id
                       AND d.status IN ('queued','claimed')) AS undelivered_count,
                  (SELECT substr(m.body,1,160) FROM session_messages m WHERE m.session_id=c.id AND m.recalled_at IS NULL ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_preview,
                  (SELECT m.author_type FROM session_messages m WHERE m.session_id=c.id AND m.recalled_at IS NULL ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_author_type,
                  (SELECT m.created_at FROM session_messages m WHERE m.session_id=c.id AND m.recalled_at IS NULL ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_at
                 FROM sessions c JOIN agent_instances a ON a.id=c.agent_instance_id
                 LEFT JOIN projects p ON p.id=c.project_id
                 LEFT JOIN tasks t ON t.id=c.task_id
                 ORDER BY c.updated_at DESC,c.id DESC",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
        };
        rows.into_iter().map(row_to_session_summary).collect()
    }

    pub async fn session_history(
        &self,
        session_id: Id,
        before_message_id: Option<Id>,
        after_message_id: Option<Id>,
        limit: i64,
    ) -> Result<SessionHistory, DomainError> {
        if before_message_id.is_some() && after_message_id.is_some() {
            return Err(DomainError::InvalidInput(
                "before_message_id and after_message_id are mutually exclusive".into(),
            ));
        }
        let session = self.get_session(session_id).await?;
        let limit = limit.clamp(1, 200);
        let fetch_limit = limit + 1;

        let mut rows = if let Some(before) = before_message_id {
            let anchor: String = sqlx::query_scalar(
                "SELECT created_at FROM session_messages WHERE id=? AND session_id=?",
            )
            .bind(before.to_string())
            .bind(session_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound("session message anchor".into()))?;
            sqlx::query(
                "SELECT m.*,
                        (SELECT d.status FROM agent_deliveries d
                         WHERE d.kind='session_message' AND d.source_id=m.id LIMIT 1) AS delivery_status
                 FROM session_messages m
                 WHERE m.session_id=? AND (m.created_at < ? OR (m.created_at = ? AND m.id < ?))
                 ORDER BY m.created_at DESC,m.id DESC LIMIT ?",
            )
            .bind(session_id.to_string())
            .bind(&anchor)
            .bind(&anchor)
            .bind(before.to_string())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
        } else if let Some(after) = after_message_id {
            let anchor: String = sqlx::query_scalar(
                "SELECT created_at FROM session_messages WHERE id=? AND session_id=?",
            )
            .bind(after.to_string())
            .bind(session_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound("session message anchor".into()))?;
            let rows = sqlx::query(
                "SELECT m.*,
                        (SELECT d.status FROM agent_deliveries d
                         WHERE d.kind='session_message' AND d.source_id=m.id LIMIT 1) AS delivery_status
                 FROM session_messages m
                 WHERE m.session_id=? AND (m.created_at > ? OR (m.created_at = ? AND m.id > ?))
                 ORDER BY m.created_at,m.id LIMIT ?",
            )
            .bind(session_id.to_string())
            .bind(&anchor)
            .bind(&anchor)
            .bind(after.to_string())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
            let has_more = rows.len() as i64 > limit;
            let messages = rows
                .into_iter()
                .take(limit as usize)
                .map(row_to_session_message)
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(SessionHistory {
                session,
                messages,
                has_more,
                next_before: None,
            });
        } else {
            sqlx::query(
                "SELECT m.*,
                        (SELECT d.status FROM agent_deliveries d
                         WHERE d.kind='session_message' AND d.source_id=m.id LIMIT 1) AS delivery_status
                 FROM session_messages m
                 WHERE m.session_id=? ORDER BY m.created_at DESC,m.id DESC LIMIT ?",
            )
            .bind(session_id.to_string())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
        };

        let has_more = rows.len() as i64 > limit;
        rows.truncate(limit as usize);
        rows.reverse();
        let messages = rows
            .into_iter()
            .map(row_to_session_message)
            .collect::<Result<Vec<_>, _>>()?;
        let next_before = if has_more {
            messages.first().map(|message| message.id)
        } else {
            None
        };
        Ok(SessionHistory {
            session,
            messages,
            has_more,
            next_before,
        })
    }

    pub async fn create_human_session_message(
        &self,
        session_id: Id,
        body: &str,
    ) -> Result<SessionMessage, DomainError> {
        self.create_human_session_message_idempotent(session_id, body, None)
            .await
    }

    pub async fn create_human_session_message_idempotent(
        &self,
        session_id: Id,
        body: &str,
        client_message_id: Option<&str>,
    ) -> Result<SessionMessage, DomainError> {
        let body = normalized_message_body(body)?;
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

        let session = self.get_session(session_id).await?;
        if session.status != "open" {
            return Err(DomainError::Conflict("session is archived".into()));
        }

        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;

        if let Some(client_id) = client_message_id.as_deref() {
            let existing = sqlx::query(
                "SELECT id,body FROM session_messages
                 WHERE session_id=? AND client_message_id=? LIMIT 1",
            )
            .bind(session_id.to_string())
            .bind(client_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
            if let Some(row) = existing {
                let id = parse_id(row.try_get("id").map_err(storage)?)?;
                let existing_body: String = row.try_get("body").map_err(storage)?;
                if existing_body != body {
                    return Err(DomainError::Conflict(
                        "client_message_id was already used for different content".into(),
                    ));
                }
                tx.commit().await.map_err(storage)?;
                return self.get_session_message(id).await;
            }
        }

        let id = Uuid::new_v4();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO session_messages(
                id,session_id,author_type,author_agent_instance_id,body,
                client_message_id,status,created_at
             ) VALUES(?,?,'human',NULL,?,?,'queued',?)",
        )
        .bind(id.to_string())
        .bind(session_id.to_string())
        .bind(&body)
        .bind(client_message_id.as_deref())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query("UPDATE sessions SET updated_at=? WHERE id=?")
            .bind(now.to_rfc3339())
            .bind(session_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        let delivery_id = enqueue_agent_delivery_tx(
            &mut tx,
            session.agent_instance_id,
            session.task_id,
            "session_message",
            id,
            json!({
                "session_id": session_id,
                "message_id": id,
                "body": body,
            }),
        )
        .await?;
        append_event_tx(
            &mut tx,
            "human",
            "local",
            "session",
            session_id,
            "session.message_queued",
            json!({
                "message_id": id,
                "agent_instance_id": session.agent_instance_id,
                "delivery_id": delivery_id,
                "client_message_id": client_message_id,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_session_message(id).await
    }

    pub async fn recall_human_session_message(
        &self,
        session_id: Id,
        message_id: Id,
    ) -> Result<SessionMessage, DomainError> {
        self.get_session(session_id).await?;
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;

        let row = sqlx::query(
            "SELECT m.author_type,m.status,m.recalled_at,
                    d.id AS delivery_id,d.status AS delivery_status
             FROM session_messages m
             LEFT JOIN agent_deliveries d
               ON d.kind='session_message' AND d.source_id=m.id
             WHERE m.id=? AND m.session_id=?",
        )
        .bind(message_id.to_string())
        .bind(session_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("session message {message_id}")))?;

        let author_type: String = row.try_get("author_type").map_err(storage)?;
        let message_status: String = row.try_get("status").map_err(storage)?;
        let recalled_at: Option<String> = row.try_get("recalled_at").map_err(storage)?;
        let delivery_id: Option<String> = row.try_get("delivery_id").map_err(storage)?;
        let delivery_status: Option<String> = row.try_get("delivery_status").map_err(storage)?;

        if recalled_at.is_some() {
            tx.commit().await.map_err(storage)?;
            return self.get_session_message(message_id).await;
        }
        if author_type != "human" {
            return Err(DomainError::Conflict(
                "only human messages can be recalled".into(),
            ));
        }
        if message_status != "queued" {
            return Err(DomainError::Conflict(
                "message has already been answered and cannot be recalled".into(),
            ));
        }
        match delivery_status.as_deref() {
            Some("queued") => {}
            Some("claimed") => {
                return Err(DomainError::Conflict(
                    "message is already being delivered to an Agent runtime".into(),
                ));
            }
            Some("delivered") => {
                return Err(DomainError::Conflict(
                    "message has already reached an Agent runtime".into(),
                ));
            }
            _ => {
                return Err(DomainError::InvalidState(
                    "message has no retractable queued delivery".into(),
                ));
            }
        }

        let delivery_id = delivery_id.expect("queued delivery has an id");
        let deleted = sqlx::query("DELETE FROM agent_deliveries WHERE id=? AND status='queued'")
            .bind(&delivery_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        if deleted.rows_affected() != 1 {
            return Err(DomainError::Conflict(
                "message delivery state changed before recall".into(),
            ));
        }

        sqlx::query("UPDATE session_messages SET recalled_at=? WHERE id=?")
            .bind(now.to_rfc3339())
            .bind(message_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        sqlx::query("UPDATE sessions SET updated_at=? WHERE id=?")
            .bind(now.to_rfc3339())
            .bind(session_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;

        append_event_tx(
            &mut tx,
            "human",
            "local",
            "session",
            session_id,
            "session.message_recalled",
            json!({
                "message_id": message_id,
                "delivery_id": delivery_id,
            }),
            None,
        )
        .await?;

        tx.commit().await.map_err(storage)?;
        self.get_session_message(message_id).await
    }

    pub async fn agent_session_inbox(
        &self,
        agent_id: Id,
    ) -> Result<Vec<SessionSummary>, DomainError> {
        Ok(self
            .list_sessions(Some(agent_id))
            .await?
            .into_iter()
            .filter(|session| session.queued_count > 0)
            .collect())
    }

    pub async fn agent_session_history(
        &self,
        session_id: Id,
        agent_id: Id,
        before_message_id: Option<Id>,
        after_message_id: Option<Id>,
        limit: i64,
    ) -> Result<SessionHistory, DomainError> {
        let session = self.get_session(session_id).await?;
        if session.agent_instance_id != agent_id {
            return Err(DomainError::Conflict(
                "session belongs to another agent instance".into(),
            ));
        }
        self.session_history(session_id, before_message_id, after_message_id, limit)
            .await
    }

    pub async fn agent_reply_session(
        &self,
        session_id: Id,
        agent_id: Id,
        body: &str,
    ) -> Result<SessionMessage, DomainError> {
        let body = normalized_message_body(body)?;
        self.get_agent(agent_id).await?;
        let session = self.get_session(session_id).await?;
        if session.agent_instance_id != agent_id {
            return Err(DomainError::Conflict(
                "session belongs to another agent instance".into(),
            ));
        }
        if session.status != "open" {
            return Err(DomainError::Conflict("session is archived".into()));
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
             WHERE agent_instance_id=? AND kind='session_message' AND status='queued'
               AND source_id IN (
                 SELECT id FROM session_messages
                 WHERE session_id=? AND author_type='human' AND status='queued' AND recalled_at IS NULL
               )",
        )
        .bind(format!("session_reply:{agent_id}"))
        .bind(now.to_rfc3339())
        .bind(agent_id.to_string())
        .bind(session_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE session_messages SET status='delivered'
             WHERE session_id=? AND author_type='human' AND status='queued' AND recalled_at IS NULL",
        )
        .bind(session_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "INSERT INTO session_messages(id,session_id,author_type,author_agent_instance_id,body,status,created_at)
             VALUES(?,?,'agent',?,?,'delivered',?)",
        )
        .bind(id.to_string())
        .bind(session_id.to_string())
        .bind(agent_id.to_string())
        .bind(&body)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query("UPDATE sessions SET updated_at=? WHERE id=?")
            .bind(now.to_rfc3339())
            .bind(session_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "session",
            session_id,
            "session.replied",
            json!({"message_id": id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_session_message(id).await
    }

    async fn get_session_message(&self, id: Id) -> Result<SessionMessage, DomainError> {
        let row = sqlx::query(
            "SELECT m.*,
                    (SELECT d.status FROM agent_deliveries d
                     WHERE d.kind='session_message' AND d.source_id=m.id LIMIT 1) AS delivery_status
             FROM session_messages m WHERE m.id=?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("session message {id}")))?;
        row_to_session_message(row)
    }

    pub async fn create_session_summary_revision(
        &self,
        input: CreateSessionSummaryRevision,
    ) -> Result<SessionSummaryRevision, DomainError> {
        self.get_session(input.session_id).await?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO session_summary_revisions(
                id,session_id,previous_revision_id,covers_until_message_id,
                goal,current_state,important_findings_json,decisions_json,
                blockers_json,unresolved_questions_json,next_steps_json,
                deterministic_facts_json,created_by,created_at
             ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(input.session_id.to_string())
        .bind(input.previous_revision_id.map(|x| x.to_string()))
        .bind(input.covers_until_message_id.map(|x| x.to_string()))
        .bind(&input.goal)
        .bind(&input.current_state)
        .bind(input.important_findings.to_string())
        .bind(input.decisions.to_string())
        .bind(input.blockers.to_string())
        .bind(input.unresolved_questions.to_string())
        .bind(input.next_steps.to_string())
        .bind(input.deterministic_facts.to_string())
        .bind(&input.created_by)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        append_event_tx(
            &mut tx,
            "system",
            &input.created_by,
            "session_summary",
            id,
            "session_summary.created",
            json!({"session_id": input.session_id}),
            None,
        )
        .await?;

        tx.commit().await.map_err(storage)?;
        self.get_session_summary_revision(id).await
    }

    pub async fn get_session_summary_revision(
        &self,
        id: Id,
    ) -> Result<SessionSummaryRevision, DomainError> {
        let row = sqlx::query("SELECT * FROM session_summary_revisions WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("session summary revision {id}")))?;
        row_to_session_summary_revision(row)
    }

    pub async fn get_latest_session_summary_revision(
        &self,
        session_id: Id,
    ) -> Result<Option<SessionSummaryRevision>, DomainError> {
        self.get_session(session_id).await?;
        let row = sqlx::query(
            "SELECT * FROM session_summary_revisions
             WHERE session_id=?
             ORDER BY created_at DESC, id DESC
             LIMIT 1",
        )
        .bind(session_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        row.map(row_to_session_summary_revision).transpose()
    }

    pub async fn list_session_summary_revisions(
        &self,
        session_id: Id,
    ) -> Result<Vec<SessionSummaryRevision>, DomainError> {
        self.get_session(session_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM session_summary_revisions
             WHERE session_id=?
             ORDER BY created_at DESC, id DESC",
        )
        .bind(session_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(row_to_session_summary_revision)
            .collect()
    }
}

fn normalized_message_body(body: &str) -> Result<String, DomainError> {
    let body = body.trim();
    if body.is_empty() || body.chars().count() > 12000 {
        return Err(DomainError::InvalidInput(
            "session message must contain 1 to 12000 characters".into(),
        ));
    }
    Ok(body.to_owned())
}

fn row_to_session(row: sqlx::sqlite::SqliteRow) -> Result<Session, DomainError> {
    Ok(Session {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        project_id: parse_opt_id(row.try_get("project_id").map_err(storage)?)?,
        task_id: parse_opt_id(row.try_get("task_id").map_err(storage)?)?,
        title: row.try_get("title").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}

fn row_to_session_summary(row: sqlx::sqlite::SqliteRow) -> Result<SessionSummary, DomainError> {
    let last_message_at: Option<String> = row.try_get("last_message_at").map_err(storage)?;
    Ok(SessionSummary {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        agent_name: row.try_get("agent_name").map_err(storage)?,
        project_id: parse_opt_id(row.try_get("project_id").map_err(storage)?)?,
        project_name: row.try_get("project_name").map_err(storage)?,
        task_id: parse_opt_id(row.try_get("task_id").map_err(storage)?)?,
        task_title: row.try_get("task_title").map_err(storage)?,
        title: row.try_get("title").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        message_count: row.try_get("message_count").map_err(storage)?,
        queued_count: row.try_get("queued_count").map_err(storage)?,
        undelivered_count: row.try_get("undelivered_count").map_err(storage)?,
        last_message_preview: row.try_get("last_message_preview").map_err(storage)?,
        last_message_author_type: row.try_get("last_message_author_type").map_err(storage)?,
        last_message_at: last_message_at.map(parse_dt).transpose()?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}

fn row_to_session_message(row: sqlx::sqlite::SqliteRow) -> Result<SessionMessage, DomainError> {
    Ok(SessionMessage {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        session_id: parse_id(row.try_get("session_id").map_err(storage)?)?,
        author_type: row.try_get("author_type").map_err(storage)?,
        author_agent_instance_id: parse_opt_id(
            row.try_get("author_agent_instance_id").map_err(storage)?,
        )?,
        body: row.try_get("body").map_err(storage)?,
        client_message_id: row.try_get("client_message_id").map_err(storage)?,
        recalled_at: parse_opt_dt(row.try_get("recalled_at").map_err(storage)?)?,
        status: row.try_get("status").map_err(storage)?,
        delivery_status: row.try_get("delivery_status").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}

fn row_to_session_summary_revision(
    row: sqlx::sqlite::SqliteRow,
) -> Result<SessionSummaryRevision, DomainError> {
    Ok(SessionSummaryRevision {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        session_id: parse_id(row.try_get("session_id").map_err(storage)?)?,
        previous_revision_id: parse_opt_id(row.try_get("previous_revision_id").map_err(storage)?)?,
        covers_until_message_id: parse_opt_id(
            row.try_get("covers_until_message_id").map_err(storage)?,
        )?,
        goal: row.try_get("goal").map_err(storage)?,
        current_state: row.try_get("current_state").map_err(storage)?,
        important_findings: parse_json(row.try_get("important_findings_json").map_err(storage)?)?,
        decisions: parse_json(row.try_get("decisions_json").map_err(storage)?)?,
        blockers: parse_json(row.try_get("blockers_json").map_err(storage)?)?,
        unresolved_questions: parse_json(
            row.try_get("unresolved_questions_json").map_err(storage)?,
        )?,
        next_steps: parse_json(row.try_get("next_steps_json").map_err(storage)?)?,
        deterministic_facts: parse_json(row.try_get("deterministic_facts_json").map_err(storage)?)?,
        created_by: row.try_get("created_by").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}
