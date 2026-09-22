use super::*;
use morrows_core::*;

fn required(value: &str, field: &str) -> Result<(), DomainError> {
    if value.trim().is_empty() {
        return Err(DomainError::InvalidInput(format!(
            "{field} cannot be empty"
        )));
    }
    Ok(())
}

impl Store {
    pub async fn create_artifact(
        &self,
        task_id: Id,
        actor: Id,
        input: CreateArtifact,
    ) -> Result<Artifact, DomainError> {
        self.get_task(task_id).await?;
        self.get_agent(actor).await?;
        required(&input.title, "title")?;
        required(&input.uri, "uri")?;
        required(&input.kind, "kind")?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query("INSERT INTO artifacts(id,task_id,created_by,title,uri,description,kind,created_at) VALUES(?,?,?,?,?,?,?,?)")
            .bind(id.to_string()).bind(task_id.to_string()).bind(actor.to_string()).bind(&input.title).bind(&input.uri).bind(&input.description).bind(&input.kind).bind(now.to_rfc3339())
            .execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor.to_string(),
            "task",
            task_id,
            "artifact.created",
            json!({"artifact_id":id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(Artifact {
            id,
            task_id,
            created_by: actor,
            content: input,
            created_at: now,
        })
    }
    pub async fn task_artifacts(&self, task_id: Id) -> Result<Vec<Artifact>, DomainError> {
        self.get_task(task_id).await?;
        let rows = sqlx::query("SELECT * FROM artifacts WHERE task_id=? ORDER BY created_at,id")
            .bind(task_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        rows.into_iter()
            .map(|r| {
                Ok(Artifact {
                    id: parse_id(r.try_get("id").map_err(storage)?)?,
                    task_id,
                    created_by: parse_id(r.try_get("created_by").map_err(storage)?)?,
                    created_at: parse_dt(r.try_get("created_at").map_err(storage)?)?,
                    content: CreateArtifact {
                        title: r.try_get("title").map_err(storage)?,
                        uri: r.try_get("uri").map_err(storage)?,
                        description: r.try_get("description").map_err(storage)?,
                        kind: r.try_get("kind").map_err(storage)?,
                    },
                })
            })
            .collect()
    }
    pub async fn create_decision(
        &self,
        task_id: Id,
        actor: Id,
        input: CreateDecision,
    ) -> Result<Decision, DomainError> {
        self.get_task(task_id).await?;
        self.get_agent(actor).await?;
        required(&input.title, "title")?;
        required(&input.rationale, "rationale")?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query("INSERT INTO decisions(id,task_id,created_by,title,rationale,created_at) VALUES(?,?,?,?,?,?)")
            .bind(id.to_string()).bind(task_id.to_string()).bind(actor.to_string()).bind(&input.title).bind(&input.rationale).bind(now.to_rfc3339())
            .execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor.to_string(),
            "task",
            task_id,
            "decision.created",
            json!({"decision_id":id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(Decision {
            id,
            task_id,
            created_by: actor,
            content: input,
            created_at: now,
        })
    }
    pub async fn task_decisions(&self, task_id: Id) -> Result<Vec<Decision>, DomainError> {
        self.get_task(task_id).await?;
        let rows = sqlx::query("SELECT * FROM decisions WHERE task_id=? ORDER BY created_at,id")
            .bind(task_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        rows.into_iter()
            .map(|r| {
                Ok(Decision {
                    id: parse_id(r.try_get("id").map_err(storage)?)?,
                    task_id,
                    created_by: parse_id(r.try_get("created_by").map_err(storage)?)?,
                    created_at: parse_dt(r.try_get("created_at").map_err(storage)?)?,
                    content: CreateDecision {
                        title: r.try_get("title").map_err(storage)?,
                        rationale: r.try_get("rationale").map_err(storage)?,
                    },
                })
            })
            .collect()
    }
    pub async fn create_thread(
        &self,
        task_id: Id,
        actor: Id,
        input: CreateThread,
    ) -> Result<MessageThread, DomainError> {
        self.get_task(task_id).await?;
        self.get_agent(actor).await?;
        required(&input.title, "title")?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO message_threads(id,task_id,created_by,title,created_at) VALUES(?,?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(task_id.to_string())
        .bind(actor.to_string())
        .bind(&input.title)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor.to_string(),
            "task",
            task_id,
            "thread.created",
            json!({"thread_id":id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(MessageThread {
            id,
            task_id,
            created_by: actor,
            title: input.title,
            created_at: now,
        })
    }
    pub async fn task_threads(&self, task_id: Id) -> Result<Vec<MessageThread>, DomainError> {
        self.get_task(task_id).await?;
        let rows =
            sqlx::query("SELECT * FROM message_threads WHERE task_id=? ORDER BY created_at,id")
                .bind(task_id.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(storage)?;
        rows.into_iter()
            .map(|r| {
                Ok(MessageThread {
                    id: parse_id(r.try_get("id").map_err(storage)?)?,
                    task_id,
                    created_by: parse_id(r.try_get("created_by").map_err(storage)?)?,
                    created_at: parse_dt(r.try_get("created_at").map_err(storage)?)?,
                    title: r.try_get("title").map_err(storage)?,
                })
            })
            .collect()
    }
    pub async fn create_message(
        &self,
        thread_id: Id,
        actor: Id,
        mut input: CreateMessage,
    ) -> Result<Message, DomainError> {
        required(&input.body, "body")?;
        required(&input.message_type, "message_type")?;
        required(&input.status, "status")?;
        if let Some(role) = &input.recipient_role {
            required(role, "recipient_role")?;
        }
        if let Some(correlation) = &input.correlation_id {
            required(correlation, "correlation_id")?;
        }
        if let Some(raw) = &input.recipient_agent_instance_id {
            let id = Uuid::parse_str(raw)
                .map_err(|_| DomainError::InvalidInput("invalid recipient UUID".into()))?;
            self.get_agent(id).await?;
            input.recipient_agent_instance_id = Some(id.to_string());
        }
        // A reply stays in its parent's thread, preserving a traversable conversation.
        if let Some(raw) = &input.reply_to_message_id {
            let id = Uuid::parse_str(raw)
                .map_err(|_| DomainError::InvalidInput("invalid reply UUID".into()))?;
            let found: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id=? AND thread_id=?)",
            )
            .bind(id.to_string())
            .bind(thread_id.to_string())
            .fetch_one(&self.pool)
            .await
            .map_err(storage)?;
            if !found {
                return Err(DomainError::InvalidInput(
                    "reply must reference a message in this thread".into(),
                ));
            }
            input.reply_to_message_id = Some(id.to_string());
        }

        self.get_agent(actor).await?;
        let task: String = sqlx::query_scalar("SELECT task_id FROM message_threads WHERE id=?")
            .bind(thread_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound("thread".into()))?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO messages(id,thread_id,created_by,body,message_type,recipient_agent_instance_id,recipient_role,reply_to_message_id,correlation_id,requires_response,status,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(thread_id.to_string())
        .bind(actor.to_string())
        .bind(&input.body)
        .bind(&input.message_type)
        .bind(&input.recipient_agent_instance_id)
        .bind(&input.recipient_role)
        .bind(&input.reply_to_message_id)
        .bind(&input.correlation_id)
        .bind(input.requires_response)
        .bind(&input.status)

        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor.to_string(),
            "task",
            parse_id(task)?,
            "message.created",
            json!({"thread_id":thread_id,"message_id":id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(Message {
            id,
            thread_id,
            created_by: actor,
            body: input.body,
            message_type: input.message_type,
            recipient_agent_instance_id: input.recipient_agent_instance_id,
            recipient_role: input.recipient_role,
            reply_to_message_id: input.reply_to_message_id,
            correlation_id: input.correlation_id,
            requires_response: input.requires_response,
            status: input.status,
            created_at: now,
        })
    }
    pub async fn thread_messages(&self, thread_id: Id) -> Result<Vec<Message>, DomainError> {
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM message_threads WHERE id=?)")
                .bind(thread_id.to_string())
                .fetch_one(&self.pool)
                .await
                .map_err(storage)?;
        if !exists {
            return Err(DomainError::NotFound("thread".into()));
        }
        let rows = sqlx::query("SELECT * FROM messages WHERE thread_id=? ORDER BY created_at,id")
            .bind(thread_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        rows.into_iter()
            .map(|r| {
                Ok(Message {
                    id: parse_id(r.try_get("id").map_err(storage)?)?,
                    thread_id,
                    created_by: parse_id(r.try_get("created_by").map_err(storage)?)?,
                    body: r.try_get("body").map_err(storage)?,
                    message_type: r.try_get("message_type").map_err(storage)?,
                    recipient_agent_instance_id: r
                        .try_get("recipient_agent_instance_id")
                        .map_err(storage)?,
                    recipient_role: r.try_get("recipient_role").map_err(storage)?,
                    reply_to_message_id: r.try_get("reply_to_message_id").map_err(storage)?,
                    correlation_id: r.try_get("correlation_id").map_err(storage)?,
                    requires_response: r.try_get("requires_response").map_err(storage)?,
                    status: r.try_get("status").map_err(storage)?,
                    created_at: parse_dt(r.try_get("created_at").map_err(storage)?)?,
                })
            })
            .collect()
    }

    /// Serialize graph edits before checking reachability, so concurrent reverse edges
    /// cannot both pass the cycle check. Dependencies point from work to prerequisites.
    pub async fn add_dependency(
        &self,
        task_id: Id,
        depends_on: Id,
        actor: Id,
    ) -> Result<TaskDependency, DomainError> {
        self.get_agent(actor).await?;
        self.get_task(task_id).await?;
        self.get_task(depends_on).await?;
        if task_id == depends_on {
            return Err(DomainError::InvalidInput("self dependency".into()));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let cycle: bool=sqlx::query_scalar("WITH RECURSIVE reachable(id) AS (SELECT depends_on_task_id FROM task_dependencies WHERE task_id=? UNION SELECT d.depends_on_task_id FROM task_dependencies d JOIN reachable r ON d.task_id=r.id) SELECT EXISTS(SELECT 1 FROM reachable WHERE id=?)")
            .bind(depends_on.to_string()).bind(task_id.to_string()).fetch_one(&mut *tx).await.map_err(storage)?;
        if cycle {
            return Err(DomainError::Conflict("dependency cycle".into()));
        }
        let now = Utc::now();
        let result=sqlx::query("INSERT OR IGNORE INTO task_dependencies(task_id,depends_on_task_id,created_by,created_at) VALUES(?,?,?,?)")
            .bind(task_id.to_string()).bind(depends_on.to_string()).bind(actor.to_string()).bind(now.to_rfc3339()).execute(&mut *tx).await.map_err(storage)?;
        if result.rows_affected() == 0 {
            return Err(DomainError::Conflict("dependency already exists".into()));
        }
        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor.to_string(),
            "task",
            task_id,
            "dependency.added",
            json!({"depends_on_task_id":depends_on}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(TaskDependency {
            task_id,
            depends_on_task_id: depends_on,
            created_by: actor,
            created_at: now,
        })
    }
    pub async fn remove_dependency(
        &self,
        task_id: Id,
        depends_on: Id,
        actor: Id,
    ) -> Result<(), DomainError> {
        self.get_agent(actor).await?;
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let result =
            sqlx::query("DELETE FROM task_dependencies WHERE task_id=? AND depends_on_task_id=?")
                .bind(task_id.to_string())
                .bind(depends_on.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
        if result.rows_affected() == 0 {
            return Err(DomainError::NotFound("dependency".into()));
        }
        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor.to_string(),
            "task",
            task_id,
            "dependency.removed",
            json!({"depends_on_task_id":depends_on}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(())
    }
    pub async fn task_dependencies(&self, task_id: Id) -> Result<Vec<TaskDependency>, DomainError> {
        self.get_task(task_id).await?;
        let rows=sqlx::query("SELECT * FROM task_dependencies WHERE task_id=? ORDER BY created_at,depends_on_task_id").bind(task_id.to_string()).fetch_all(&self.pool).await.map_err(storage)?;
        rows.into_iter()
            .map(|r| {
                Ok(TaskDependency {
                    task_id,
                    depends_on_task_id: parse_id(
                        r.try_get("depends_on_task_id").map_err(storage)?,
                    )?,
                    created_by: parse_id(r.try_get("created_by").map_err(storage)?)?,
                    created_at: parse_dt(r.try_get("created_at").map_err(storage)?)?,
                })
            })
            .collect()
    }

    /// Freeze a continuation against an immutable context revision and release the
    /// assignment in one transaction. Stop every live run on that assignment so no
    /// sibling session can later complete the task after ownership has transferred.
    pub async fn create_handoff(
        &self,
        run_id: Id,
        actor: Id,
        input: CreateHandoff,
    ) -> Result<Handoff, DomainError> {
        required(&input.summary, "summary")?;
        if input.remaining.is_empty() {
            return Err(DomainError::InvalidInput(
                "handoff needs remaining work".into(),
            ));
        }
        for item in input
            .completed
            .iter()
            .chain(&input.remaining)
            .chain(&input.blockers)
        {
            required(item, "handoff item")?;
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row = sqlx::query("SELECT * FROM runs WHERE id=?")
            .bind(run_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound("run".into()))?;
        let run = row_to_run(row)?;
        if run.agent_instance_id != actor || !["running", "paused"].contains(&run.status.as_str()) {
            return Err(DomainError::Conflict(
                "handoff requires a live run owned by this agent".into(),
            ));
        }
        let a = row_to_assignment(
            sqlx::query("SELECT * FROM assignments WHERE id=?")
                .bind(run.assignment_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?,
        )?;
        if a.status != "active" || a.expires_at <= Utc::now() {
            return Err(DomainError::Conflict("assignment is not active".into()));
        }
        let context: Option<String> =
            sqlx::query_scalar("SELECT current_context_revision_id FROM tasks WHERE id=?")
                .bind(run.task_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?;
        let context_revision_id = parse_id(context.ok_or_else(|| {
            DomainError::InvalidInput("create task context before handoff".into())
        })?)?;
        let id = Uuid::new_v4();
        let now = Utc::now();
        // Validate references within this task; links are foreign-key backed, not
        // unchecked identifiers hidden only inside a JSON document.
        for (table, ids) in [
            ("artifacts", &input.artifact_ids),
            ("decisions", &input.decision_ids),
        ] {
            let mut seen = std::collections::HashSet::new();
            for raw in ids {
                let ref_id = Uuid::parse_str(raw)
                    .map_err(|_| DomainError::InvalidInput("invalid reference UUID".into()))?;
                if !seen.insert(ref_id) {
                    return Err(DomainError::InvalidInput(
                        "duplicate handoff reference".into(),
                    ));
                }
                let found: bool = sqlx::query_scalar(&format!(
                    "SELECT EXISTS(SELECT 1 FROM {table} WHERE id=? AND task_id=?)"
                ))
                .bind(ref_id.to_string())
                .bind(run.task_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?;
                if !found {
                    return Err(DomainError::InvalidInput(format!(
                        "{table} reference is not in this task"
                    )));
                }
            }
        }
        sqlx::query("INSERT INTO handoffs(id,task_id,source_run_id,created_by,context_revision_id,content_json,created_at) VALUES(?,?,?,?,?,?,?)")
            .bind(id.to_string()).bind(run.task_id.to_string()).bind(run_id.to_string()).bind(actor.to_string()).bind(context_revision_id.to_string()).bind(serde_json::to_string(&input).map_err(storage)?).bind(now.to_rfc3339()).execute(&mut *tx).await.map_err(storage)?;
        for (table, col, ids) in [
            ("handoff_artifacts", "artifact_id", &input.artifact_ids),
            ("handoff_decisions", "decision_id", &input.decision_ids),
        ] {
            for raw in ids {
                sqlx::query(&format!(
                    "INSERT INTO {table}(handoff_id,{col}) VALUES(?,?)"
                ))
                .bind(id.to_string())
                .bind(Uuid::parse_str(raw).map_err(storage)?.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            }
        }
        sqlx::query("UPDATE runs SET status='handed_off',stop_reason='handoff',ended_at=? WHERE assignment_id=? AND status IN ('running','paused')").bind(now.to_rfc3339()).bind(a.id.to_string()).execute(&mut *tx).await.map_err(storage)?;
        sqlx::query("UPDATE assignments SET status='released',released_at=?,release_reason='handoff' WHERE id=?").bind(now.to_rfc3339()).bind(a.id.to_string()).execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(&mut tx,"agent_instance",&actor.to_string(),"task",run.task_id,"handoff.created",json!({"handoff_id":id,"source_run_id":run_id,"assignment_id":a.id,"context_revision_id":context_revision_id}),None).await?;
        tx.commit().await.map_err(storage)?;
        Ok(Handoff {
            id,
            task_id: run.task_id,
            source_run_id: run_id,
            status: "pending".into(),
            accepted_by_run_id: None,
            created_by: actor,
            context_revision_id,
            content: input,
            created_at: now,
        })
    }
    /// Serialize acceptance with run/lease mutations. Validation, the pending-only
    /// transition, and its audit event share a transaction, so competing targets
    /// cannot both accept and failed attempts leave neither state nor event changes.
    pub async fn accept_handoff(
        &self,
        id: Id,
        target_run_id: Id,
        actor: Id,
    ) -> Result<Handoff, DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let mut handoff = row_to_handoff(
            sqlx::query("SELECT * FROM handoffs WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound("handoff".into()))?,
        )?;
        if handoff.status != "pending" {
            return Err(DomainError::Conflict("handoff already accepted".into()));
        }
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(target_run_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound("target run".into()))?,
        )?;
        if run.task_id != handoff.task_id
            || run.agent_instance_id != actor
            || !["running", "paused"].contains(&run.status.as_str())
            || run.id == handoff.source_run_id
        {
            return Err(DomainError::Conflict(
                "acceptance requires a live same-task target run owned by this agent".into(),
            ));
        }
        let assignment = row_to_assignment(
            sqlx::query("SELECT * FROM assignments WHERE id=?")
                .bind(run.assignment_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?,
        )?;
        if assignment.status != "active"
            || assignment.expires_at <= Utc::now()
            || assignment.agent_instance_id != actor
            || assignment.task_id != handoff.task_id
        {
            return Err(DomainError::Conflict(
                "target assignment is not active".into(),
            ));
        }
        sqlx::query("UPDATE handoffs SET status='accepted',accepted_by_run_id=? WHERE id=? AND status='pending'")
            .bind(target_run_id.to_string()).bind(id.to_string()).execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(&mut tx, "agent_instance", &actor.to_string(), "task", handoff.task_id,
            "handoff.accepted", json!({"handoff_id":id,"source_run_id":handoff.source_run_id,"accepted_by_run_id":target_run_id}), None).await?;
        tx.commit().await.map_err(storage)?;
        handoff.status = "accepted".into();
        handoff.accepted_by_run_id = Some(target_run_id);
        Ok(handoff)
    }
    pub async fn task_handoffs(&self, task_id: Id) -> Result<Vec<Handoff>, DomainError> {
        self.get_task(task_id).await?;
        sqlx::query("SELECT * FROM handoffs WHERE task_id=? ORDER BY created_at,id")
            .bind(task_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
            .into_iter()
            .map(row_to_handoff)
            .collect()
    }
    pub async fn get_handoff(&self, id: Id) -> Result<HandoffContext, DomainError> {
        let row = sqlx::query("SELECT * FROM handoffs WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound("handoff".into()))?;
        let handoff = row_to_handoff(row)?;
        let context = self
            .get_context_revision(handoff.context_revision_id)
            .await?;
        let artifacts = self
            .task_artifacts(handoff.task_id)
            .await?
            .into_iter()
            .filter(|a| {
                handoff
                    .content
                    .artifact_ids
                    .iter()
                    .any(|raw| Uuid::parse_str(raw).ok() == Some(a.id))
            })
            .collect();
        let decisions = self
            .task_decisions(handoff.task_id)
            .await?
            .into_iter()
            .filter(|d| {
                handoff
                    .content
                    .decision_ids
                    .iter()
                    .any(|raw| Uuid::parse_str(raw).ok() == Some(d.id))
            })
            .collect();
        Ok(HandoffContext {
            handoff,
            context,
            artifacts,
            decisions,
        })
    }
    pub async fn task_collaboration(&self, task_id: Id) -> Result<Collaboration, DomainError> {
        let threads = self.task_threads(task_id).await?;
        let mut messages = Vec::new();
        for thread in &threads {
            messages.extend(self.thread_messages(thread.id).await?);
        }
        Ok(Collaboration {
            handoffs: self.task_handoffs(task_id).await?,
            artifacts: self.task_artifacts(task_id).await?,
            decisions: self.task_decisions(task_id).await?,
            threads,
            messages,
            dependencies: self.task_dependencies(task_id).await?,
        })
    }
}
fn row_to_handoff(r: sqlx::sqlite::SqliteRow) -> Result<Handoff, DomainError> {
    Ok(Handoff {
        id: parse_id(r.try_get("id").map_err(storage)?)?,
        task_id: parse_id(r.try_get("task_id").map_err(storage)?)?,
        source_run_id: parse_id(r.try_get("source_run_id").map_err(storage)?)?,
        status: r.try_get("status").map_err(storage)?,
        accepted_by_run_id: r
            .try_get::<Option<String>, _>("accepted_by_run_id")
            .map_err(storage)?
            .map(parse_id)
            .transpose()?,
        created_by: parse_id(r.try_get("created_by").map_err(storage)?)?,
        context_revision_id: parse_id(r.try_get("context_revision_id").map_err(storage)?)?,
        content: serde_json::from_str(&r.try_get::<String, _>("content_json").map_err(storage)?)
            .map_err(storage)?,
        created_at: parse_dt(r.try_get("created_at").map_err(storage)?)?,
    })
}
