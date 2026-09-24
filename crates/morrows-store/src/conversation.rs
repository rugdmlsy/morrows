use super::*;
use crate::delivery::enqueue_agent_delivery_tx;
use morrows_core::{
    Conversation, ConversationHistory, ConversationMessage, ConversationSummary,
    ConversationSummaryRevision, CreateConversation, CreateConversationSummaryRevision,
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

    pub async fn create_conversation(
        &self,
        input: CreateConversation,
    ) -> Result<Conversation, DomainError> {
        let agent = self.get_agent(input.agent_instance_id).await?;
        let title = if input.title.trim().is_empty() {
            format!("Conversation with {}", agent.name)
        } else {
            input.title.trim().to_owned()
        };
        if title.chars().count() > 200 {
            return Err(DomainError::InvalidInput(
                "conversation title must be at most 200 characters".into(),
            ));
        }
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO conversations(id,agent_instance_id,title,status,created_at,updated_at)
             VALUES(?,?,?,'open',?,?)",
        )
        .bind(id.to_string())
        .bind(input.agent_instance_id.to_string())
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
            "conversation",
            id,
            "conversation.created",
            json!({"agent_instance_id": input.agent_instance_id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_conversation(id).await
    }

    pub async fn get_conversation(&self, id: Id) -> Result<Conversation, DomainError> {
        let row = sqlx::query("SELECT * FROM conversations WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("conversation {id}")))?;
        row_to_conversation(row)
    }

    pub async fn list_conversations(
        &self,
        agent_id: Option<Id>,
    ) -> Result<Vec<ConversationSummary>, DomainError> {
        if let Some(id) = agent_id {
            self.get_agent(id).await?;
        }
        let rows = if let Some(id) = agent_id {
            sqlx::query(
                "SELECT c.*,a.name AS agent_name,
                  (SELECT COUNT(*) FROM conversation_messages m WHERE m.conversation_id=c.id) AS message_count,
                  (SELECT COUNT(*) FROM conversation_messages m WHERE m.conversation_id=c.id AND m.author_type='human' AND m.status='queued') AS queued_count,
                  (SELECT COUNT(*) FROM agent_deliveries d
                     JOIN conversation_messages dm ON dm.id=d.source_id
                     WHERE d.kind='conversation_message' AND dm.conversation_id=c.id
                       AND d.status IN ('queued','claimed')) AS undelivered_count,
                  (SELECT substr(m.body,1,160) FROM conversation_messages m WHERE m.conversation_id=c.id ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_preview,
                  (SELECT m.author_type FROM conversation_messages m WHERE m.conversation_id=c.id ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_author_type,
                  (SELECT m.created_at FROM conversation_messages m WHERE m.conversation_id=c.id ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_at
                 FROM conversations c JOIN agent_instances a ON a.id=c.agent_instance_id
                 WHERE c.agent_instance_id=? ORDER BY c.updated_at DESC,c.id DESC",
            )
            .bind(id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
        } else {
            sqlx::query(
                "SELECT c.*,a.name AS agent_name,
                  (SELECT COUNT(*) FROM conversation_messages m WHERE m.conversation_id=c.id) AS message_count,
                  (SELECT COUNT(*) FROM conversation_messages m WHERE m.conversation_id=c.id AND m.author_type='human' AND m.status='queued') AS queued_count,
                  (SELECT COUNT(*) FROM agent_deliveries d
                     JOIN conversation_messages dm ON dm.id=d.source_id
                     WHERE d.kind='conversation_message' AND dm.conversation_id=c.id
                       AND d.status IN ('queued','claimed')) AS undelivered_count,
                  (SELECT substr(m.body,1,160) FROM conversation_messages m WHERE m.conversation_id=c.id ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_preview,
                  (SELECT m.author_type FROM conversation_messages m WHERE m.conversation_id=c.id ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_author_type,
                  (SELECT m.created_at FROM conversation_messages m WHERE m.conversation_id=c.id ORDER BY m.created_at DESC,m.id DESC LIMIT 1) AS last_message_at
                 FROM conversations c JOIN agent_instances a ON a.id=c.agent_instance_id
                 ORDER BY c.updated_at DESC,c.id DESC",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
        };
        rows.into_iter().map(row_to_conversation_summary).collect()
    }

    pub async fn conversation_history(
        &self,
        conversation_id: Id,
        before_message_id: Option<Id>,
        after_message_id: Option<Id>,
        limit: i64,
    ) -> Result<ConversationHistory, DomainError> {
        if before_message_id.is_some() && after_message_id.is_some() {
            return Err(DomainError::InvalidInput(
                "before_message_id and after_message_id are mutually exclusive".into(),
            ));
        }
        let conversation = self.get_conversation(conversation_id).await?;
        let limit = limit.clamp(1, 200);
        let fetch_limit = limit + 1;

        let mut rows = if let Some(before) = before_message_id {
            let anchor: String = sqlx::query_scalar(
                "SELECT created_at FROM conversation_messages WHERE id=? AND conversation_id=?",
            )
            .bind(before.to_string())
            .bind(conversation_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound("conversation message anchor".into()))?;
            sqlx::query(
                "SELECT m.*,
                        (SELECT d.status FROM agent_deliveries d
                         WHERE d.kind='conversation_message' AND d.source_id=m.id LIMIT 1) AS delivery_status
                 FROM conversation_messages m
                 WHERE m.conversation_id=? AND (m.created_at < ? OR (m.created_at = ? AND m.id < ?))
                 ORDER BY m.created_at DESC,m.id DESC LIMIT ?",
            )
            .bind(conversation_id.to_string())
            .bind(&anchor)
            .bind(&anchor)
            .bind(before.to_string())
            .bind(fetch_limit)
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
        } else if let Some(after) = after_message_id {
            let anchor: String = sqlx::query_scalar(
                "SELECT created_at FROM conversation_messages WHERE id=? AND conversation_id=?",
            )
            .bind(after.to_string())
            .bind(conversation_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound("conversation message anchor".into()))?;
            let rows = sqlx::query(
                "SELECT m.*,
                        (SELECT d.status FROM agent_deliveries d
                         WHERE d.kind='conversation_message' AND d.source_id=m.id LIMIT 1) AS delivery_status
                 FROM conversation_messages m
                 WHERE m.conversation_id=? AND (m.created_at > ? OR (m.created_at = ? AND m.id > ?))
                 ORDER BY m.created_at,m.id LIMIT ?",
            )
            .bind(conversation_id.to_string())
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
                .map(row_to_conversation_message)
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(ConversationHistory {
                conversation,
                messages,
                has_more,
                next_before: None,
            });
        } else {
            sqlx::query(
                "SELECT m.*,
                        (SELECT d.status FROM agent_deliveries d
                         WHERE d.kind='conversation_message' AND d.source_id=m.id LIMIT 1) AS delivery_status
                 FROM conversation_messages m
                 WHERE m.conversation_id=? ORDER BY m.created_at DESC,m.id DESC LIMIT ?",
            )
            .bind(conversation_id.to_string())
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
            .map(row_to_conversation_message)
            .collect::<Result<Vec<_>, _>>()?;
        let next_before = if has_more {
            messages.first().map(|message| message.id)
        } else {
            None
        };
        Ok(ConversationHistory {
            conversation,
            messages,
            has_more,
            next_before,
        })
    }

    pub async fn create_human_conversation_message(
        &self,
        conversation_id: Id,
        body: &str,
    ) -> Result<ConversationMessage, DomainError> {
        let body = normalized_message_body(body)?;
        let conversation = self.get_conversation(conversation_id).await?;
        if conversation.status != "open" {
            return Err(DomainError::Conflict("conversation is archived".into()));
        }
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO conversation_messages(id,conversation_id,author_type,author_agent_instance_id,body,status,created_at)
             VALUES(?,?,'human',NULL,?,'queued',?)",
        )
        .bind(id.to_string())
        .bind(conversation_id.to_string())
        .bind(&body)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query("UPDATE conversations SET updated_at=? WHERE id=?")
            .bind(now.to_rfc3339())
            .bind(conversation_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        let delivery_id = enqueue_agent_delivery_tx(
            &mut tx,
            conversation.agent_instance_id,
            None,
            "conversation_message",
            id,
            json!({
                "conversation_id": conversation_id,
                "message_id": id,
                "body": body,
            }),
        )
        .await?;
        append_event_tx(
            &mut tx,
            "human",
            "local",
            "conversation",
            conversation_id,
            "conversation.message_queued",
            json!({
                "message_id": id,
                "agent_instance_id": conversation.agent_instance_id,
                "delivery_id": delivery_id,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_conversation_message(id).await
    }

    pub async fn agent_conversation_inbox(
        &self,
        agent_id: Id,
    ) -> Result<Vec<ConversationSummary>, DomainError> {
        Ok(self
            .list_conversations(Some(agent_id))
            .await?
            .into_iter()
            .filter(|conversation| conversation.queued_count > 0)
            .collect())
    }

    pub async fn agent_conversation_history(
        &self,
        conversation_id: Id,
        agent_id: Id,
        before_message_id: Option<Id>,
        after_message_id: Option<Id>,
        limit: i64,
    ) -> Result<ConversationHistory, DomainError> {
        let conversation = self.get_conversation(conversation_id).await?;
        if conversation.agent_instance_id != agent_id {
            return Err(DomainError::Conflict(
                "conversation belongs to another agent instance".into(),
            ));
        }
        self.conversation_history(conversation_id, before_message_id, after_message_id, limit)
            .await
    }

    pub async fn agent_reply_conversation(
        &self,
        conversation_id: Id,
        agent_id: Id,
        body: &str,
    ) -> Result<ConversationMessage, DomainError> {
        let body = normalized_message_body(body)?;
        self.get_agent(agent_id).await?;
        let conversation = self.get_conversation(conversation_id).await?;
        if conversation.agent_instance_id != agent_id {
            return Err(DomainError::Conflict(
                "conversation belongs to another agent instance".into(),
            ));
        }
        if conversation.status != "open" {
            return Err(DomainError::Conflict("conversation is archived".into()));
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
             WHERE agent_instance_id=? AND kind='conversation_message' AND status='queued'
               AND source_id IN (
                 SELECT id FROM conversation_messages
                 WHERE conversation_id=? AND author_type='human' AND status='queued'
               )",
        )
        .bind(format!("conversation_reply:{agent_id}"))
        .bind(now.to_rfc3339())
        .bind(agent_id.to_string())
        .bind(conversation_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE conversation_messages SET status='delivered'
             WHERE conversation_id=? AND author_type='human' AND status='queued'",
        )
        .bind(conversation_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "INSERT INTO conversation_messages(id,conversation_id,author_type,author_agent_instance_id,body,status,created_at)
             VALUES(?,?,'agent',?,?,'delivered',?)",
        )
        .bind(id.to_string())
        .bind(conversation_id.to_string())
        .bind(agent_id.to_string())
        .bind(&body)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query("UPDATE conversations SET updated_at=? WHERE id=?")
            .bind(now.to_rfc3339())
            .bind(conversation_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "conversation",
            conversation_id,
            "conversation.replied",
            json!({"message_id": id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_conversation_message(id).await
    }

    async fn get_conversation_message(&self, id: Id) -> Result<ConversationMessage, DomainError> {
        let row = sqlx::query(
            "SELECT m.*,
                    (SELECT d.status FROM agent_deliveries d
                     WHERE d.kind='conversation_message' AND d.source_id=m.id LIMIT 1) AS delivery_status
             FROM conversation_messages m WHERE m.id=?",
        )
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("conversation message {id}")))?;
        row_to_conversation_message(row)
    }

    pub async fn create_conversation_summary_revision(
        &self,
        input: CreateConversationSummaryRevision,
    ) -> Result<ConversationSummaryRevision, DomainError> {
        self.get_conversation(input.conversation_id).await?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO conversation_summary_revisions(
                id,conversation_id,previous_revision_id,covers_until_message_id,
                goal,current_state,important_findings_json,decisions_json,
                blockers_json,unresolved_questions_json,next_steps_json,
                deterministic_facts_json,created_by,created_at
             ) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(input.conversation_id.to_string())
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
            "conversation_summary",
            id,
            "conversation_summary.created",
            json!({"conversation_id": input.conversation_id}),
            None,
        )
        .await?;

        tx.commit().await.map_err(storage)?;
        self.get_conversation_summary_revision(id).await
    }

    pub async fn get_conversation_summary_revision(
        &self,
        id: Id,
    ) -> Result<ConversationSummaryRevision, DomainError> {
        let row = sqlx::query("SELECT * FROM conversation_summary_revisions WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("conversation summary revision {id}")))?;
        row_to_conversation_summary_revision(row)
    }

    pub async fn get_latest_conversation_summary_revision(
        &self,
        conversation_id: Id,
    ) -> Result<Option<ConversationSummaryRevision>, DomainError> {
        self.get_conversation(conversation_id).await?;
        let row = sqlx::query(
            "SELECT * FROM conversation_summary_revisions
             WHERE conversation_id=?
             ORDER BY created_at DESC, id DESC
             LIMIT 1",
        )
        .bind(conversation_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        row.map(row_to_conversation_summary_revision).transpose()
    }

    pub async fn list_conversation_summary_revisions(
        &self,
        conversation_id: Id,
    ) -> Result<Vec<ConversationSummaryRevision>, DomainError> {
        self.get_conversation(conversation_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM conversation_summary_revisions
             WHERE conversation_id=?
             ORDER BY created_at DESC, id DESC",
        )
        .bind(conversation_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_conversation_summary_revision).collect()
    }
}

fn normalized_message_body(body: &str) -> Result<String, DomainError> {
    let body = body.trim();
    if body.is_empty() || body.chars().count() > 12000 {
        return Err(DomainError::InvalidInput(
            "conversation message must contain 1 to 12000 characters".into(),
        ));
    }
    Ok(body.to_owned())
}

fn row_to_conversation(row: sqlx::sqlite::SqliteRow) -> Result<Conversation, DomainError> {
    Ok(Conversation {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        title: row.try_get("title").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}

fn row_to_conversation_summary(
    row: sqlx::sqlite::SqliteRow,
) -> Result<ConversationSummary, DomainError> {
    let last_message_at: Option<String> = row.try_get("last_message_at").map_err(storage)?;
    Ok(ConversationSummary {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        agent_name: row.try_get("agent_name").map_err(storage)?,
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

fn row_to_conversation_message(
    row: sqlx::sqlite::SqliteRow,
) -> Result<ConversationMessage, DomainError> {
    Ok(ConversationMessage {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        conversation_id: parse_id(row.try_get("conversation_id").map_err(storage)?)?,
        author_type: row.try_get("author_type").map_err(storage)?,
        author_agent_instance_id: parse_opt_id(
            row.try_get("author_agent_instance_id").map_err(storage)?,
        )?,
        body: row.try_get("body").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        delivery_status: row.try_get("delivery_status").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}

fn row_to_conversation_summary_revision(
    row: sqlx::sqlite::SqliteRow,
) -> Result<ConversationSummaryRevision, DomainError> {
    Ok(ConversationSummaryRevision {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        conversation_id: parse_id(row.try_get("conversation_id").map_err(storage)?)?,
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
        deterministic_facts: parse_json(
            row.try_get("deterministic_facts_json").map_err(storage)?,
        )?,
        created_by: row.try_get("created_by").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}
