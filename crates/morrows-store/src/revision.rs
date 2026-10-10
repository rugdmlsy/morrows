use super::*;
use crate::delivery::enqueue_agent_delivery_tx;
use morrows_core::{TaskRevisionAck, TaskRevisionDraft, TaskSpec};

async fn spec_conn(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: Id,
) -> Result<(TaskSpec, i64, String, Option<String>, String), DomainError> {
    let row = sqlx::query("SELECT * FROM tasks WHERE id=?")
        .bind(task_id.to_string())
        .fetch_optional(&mut **tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?;
    let task = row_to_task(row)?;
    let context = sqlx::query("SELECT goal,constraints_json FROM context_revisions WHERE id=(SELECT current_context_revision_id FROM tasks WHERE id=?)")
        .bind(task_id.to_string()).fetch_optional(&mut **tx).await.map_err(storage)?;
    let (goal, scope) = if let Some(row) = context {
        let goal: String = row.try_get("goal").map_err(storage)?;
        let constraints: Value = serde_json::from_str(
            &row.try_get::<String, _>("constraints_json")
                .map_err(storage)?,
        )
        .map_err(storage)?;
        (
            goal,
            constraints
                .get("task_scope")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        )
    } else {
        (String::new(), String::new())
    };
    let current_version: i64 = sqlx::query_scalar("SELECT spec_version FROM tasks WHERE id=?")
        .bind(task_id.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage)?;
    Ok((
        TaskSpec {
            title: task.title,
            description: task.description,
            goal,
            scope,
            acceptance_criteria: task.acceptance_criteria,
        },
        current_version,
        task.state.to_string(),
        task.current_context_revision_id.map(|x| x.to_string()),
        task.owner_actor_id,
    ))
}

pub(super) async fn baseline_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: Id,
) -> Result<(), DomainError> {
    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task_revisions WHERE task_id=?)")
            .bind(task_id.to_string())
            .fetch_one(&mut **tx)
            .await
            .map_err(storage)?;
    if exists {
        return Ok(());
    }
    let (spec, version, _, context_id, owner) = spec_conn(tx, task_id).await?;
    let now = Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO task_revisions(id,task_id,version,base_version,status,author_actor_id,reason,before_json,after_json,context_revision_before,context_revision_after,created_at,resolved_at) VALUES(?,?,?,0,'applied',?,'Initial task specification',?,?,?,?,?,?)")
        .bind(Uuid::new_v4().to_string()).bind(task_id.to_string()).bind(version).bind(owner)
        .bind(serde_json::to_string(&spec).map_err(storage)?).bind(serde_json::to_string(&spec).map_err(storage)?)
        .bind(&context_id).bind(&context_id).bind(&now).bind(&now)
        .execute(&mut **tx).await.map_err(storage)?;
    Ok(())
}

fn patched(old: &TaskSpec, draft: &TaskRevisionDraft) -> Result<TaskSpec, DomainError> {
    if draft.reason.trim().is_empty() {
        return Err(DomainError::InvalidInput(
            "revision reason is required".into(),
        ));
    }
    let spec = TaskSpec {
        title: draft.title.clone().unwrap_or_else(|| old.title.clone()),
        description: draft
            .description
            .clone()
            .unwrap_or_else(|| old.description.clone()),
        goal: draft.goal.clone().unwrap_or_else(|| old.goal.clone()),
        scope: draft.scope.clone().unwrap_or_else(|| old.scope.clone()),
        acceptance_criteria: draft
            .acceptance_criteria
            .clone()
            .unwrap_or_else(|| old.acceptance_criteria.clone()),
    };
    if spec.title.trim().is_empty() || spec.title.len() > 500 {
        return Err(DomainError::InvalidInput(
            "task title must contain 1..500 characters".into(),
        ));
    }
    if spec.description.len() > 100_000 || spec.goal.len() > 100_000 || spec.scope.len() > 100_000 {
        return Err(DomainError::InvalidInput(
            "task fields cannot exceed 100000 bytes".into(),
        ));
    }
    if draft.acceptance_criteria.is_some() {
        morrows_core::validate_acceptance(&spec.acceptance_criteria)?;
    }
    if &spec == old {
        return Err(DomainError::InvalidInput("revision has no changes".into()));
    }
    Ok(spec)
}
fn diff(before: &TaskSpec, after: &TaskSpec) -> Value {
    let mut changes = serde_json::Map::new();
    for (name, old, new) in [
        ("title", json!(before.title), json!(after.title)),
        (
            "description",
            json!(before.description),
            json!(after.description),
        ),
        ("goal", json!(before.goal), json!(after.goal)),
        ("scope", json!(before.scope), json!(after.scope)),
        (
            "acceptance_criteria",
            json!(before.acceptance_criteria),
            json!(after.acceptance_criteria),
        ),
    ] {
        if old != new {
            changes.insert(name.into(), json!({"before":old,"after":new}));
        }
    }
    Value::Object(changes)
}
fn revision_json(row: sqlx::sqlite::SqliteRow) -> Result<Value, DomainError> {
    let before: Value =
        serde_json::from_str(&row.try_get::<String, _>("before_json").map_err(storage)?)
            .map_err(storage)?;
    let after: Value =
        serde_json::from_str(&row.try_get::<String, _>("after_json").map_err(storage)?)
            .map_err(storage)?;
    let plan: Option<String> = row.try_get("ack_plan_json").map_err(storage)?;
    Ok(json!({"id":row.try_get::<String,_>("id").map_err(storage)?,
        "task_id":row.try_get::<String,_>("task_id").map_err(storage)?,
        "version":row.try_get::<i64,_>("version").map_err(storage)?,
        "base_version":row.try_get::<i64,_>("base_version").map_err(storage)?,
        "status":row.try_get::<String,_>("status").map_err(storage)?,
        "author_actor_id":row.try_get::<String,_>("author_actor_id").map_err(storage)?,
        "reason":row.try_get::<String,_>("reason").map_err(storage)?,
        "before":before,"after":after,"diff":diff(&serde_json::from_value::<TaskSpec>(before).map_err(storage)?,&serde_json::from_value::<TaskSpec>(after).map_err(storage)?),
        "context_revision_before":row.try_get::<Option<String>,_>("context_revision_before").map_err(storage)?,
        "context_revision_after":row.try_get::<Option<String>,_>("context_revision_after").map_err(storage)?,
        "ack_run_id":row.try_get::<Option<String>,_>("ack_run_id").map_err(storage)?,
        "ack_agent_id":row.try_get::<Option<String>,_>("ack_agent_id").map_err(storage)?,
        "ack_impact":row.try_get::<Option<String>,_>("ack_impact").map_err(storage)?,
        "updated_plan":plan.map(|s|serde_json::from_str::<Value>(&s).unwrap_or(Value::Null)),
        "created_at":row.try_get::<String,_>("created_at").map_err(storage)?,
        "resolved_at":row.try_get::<Option<String>,_>("resolved_at").map_err(storage)?
    }))
}

async fn apply_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: Id,
    revision_id: Id,
    agent: Option<(Id, Id, &str, &[String])>,
) -> Result<(), DomainError> {
    let row = sqlx::query("SELECT * FROM task_revisions WHERE id=? AND task_id=?")
        .bind(revision_id.to_string())
        .bind(task_id.to_string())
        .fetch_one(&mut **tx)
        .await
        .map_err(storage)?;
    let rev = revision_json(row)?;
    let version = rev["version"]
        .as_i64()
        .ok_or_else(|| DomainError::InvalidInput("invalid revision".into()))?;
    let spec: TaskSpec = serde_json::from_value(rev["after"].clone()).map_err(storage)?;
    let (_, current_version, task_state, current_context, _) = spec_conn(tx, task_id).await?;
    if task_state == "done" || task_state == "cancelled" || rev["base_version"] != current_version {
        return Err(DomainError::Conflict(
            "task was completed/cancelled or revision base became stale".into(),
        ));
    }
    if rev["context_revision_before"].as_str() != current_context.as_deref() {
        return Err(DomainError::Conflict("Task context changed after revision proposal; reject and re-propose on the latest context".into()));
    }
    let latest = sqlx::query(
        "SELECT * FROM context_revisions WHERE task_id=? ORDER BY version DESC LIMIT 1",
    )
    .bind(task_id.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?;
    let (parent, context_version, background, summary, mut constraints) = if let Some(row) = latest
    {
        let ctx = row_to_context_revision(row)?;
        (
            Some(ctx.id.to_string()),
            ctx.version + 1,
            ctx.background,
            ctx.current_summary,
            ctx.constraints,
        )
    } else {
        (None, 1, String::new(), String::new(), json!({}))
    };
    if !constraints.is_object() {
        constraints = json!({});
    }
    if let Some(map) = constraints.as_object_mut() {
        map.remove("acceptance_criteria");
        map.remove("freeze_requires");
        map.insert("task_scope".into(), json!(spec.scope));
    }
    let new_context = Uuid::new_v4();
    let now = Utc::now().to_rfc3339();
    let author = rev["author_actor_id"].as_str().unwrap_or("system:revision");
    sqlx::query("INSERT INTO context_revisions(id,task_id,version,parent_revision_id,goal,background,constraints_json,current_summary,created_by_actor_id,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
      .bind(new_context.to_string()).bind(task_id.to_string()).bind(context_version).bind(parent)
      .bind(&spec.goal).bind(background).bind(constraints.to_string()).bind(summary)
      .bind(author).bind(&now).execute(&mut **tx).await.map_err(storage)?;
    sqlx::query("UPDATE tasks SET title=?,description=?,acceptance_criteria_json=?,acceptance_version=acceptance_version+1,spec_version=?,current_context_revision_id=?,updated_at=? WHERE id=? AND spec_version=? AND state NOT IN ('done','cancelled')")
      .bind(&spec.title).bind(&spec.description).bind(serde_json::to_string(&spec.acceptance_criteria).map_err(storage)?)
      .bind(version).bind(new_context.to_string()).bind(&now).bind(task_id.to_string()).bind(current_version)
      .execute(&mut **tx).await.map_err(storage)?;
    let (run, agent_id, impact, plan) = if let Some((run, agent_id, impact, plan)) = agent {
        (
            Some(run.to_string()),
            Some(agent_id.to_string()),
            Some(impact.to_owned()),
            Some(json!(plan).to_string()),
        )
    } else {
        (None, None, None, None)
    };
    sqlx::query("UPDATE task_revisions SET status='applied',resolved_at=?,context_revision_after=?,ack_run_id=?,ack_agent_id=?,ack_impact=?,ack_plan_json=? WHERE id=?")
       .bind(&now).bind(new_context.to_string()).bind(run).bind(agent_id).bind(impact).bind(plan)
       .bind(revision_id.to_string()).execute(&mut **tx).await.map_err(storage)?;
    // The old receipts/reviews remain immutable history, but their version/context no longer match.
    // A continuing Run is explicitly repinned after the executor's acknowledgment.
    sqlx::query("INSERT INTO run_context_revisions(run_id,context_revision_id,pinned_at) SELECT id,?,? FROM runs WHERE task_id=? AND status IN ('running','paused') ON CONFLICT(run_id) DO UPDATE SET context_revision_id=excluded.context_revision_id,pinned_at=excluded.pinned_at")
      .bind(new_context.to_string()).bind(&now).bind(task_id.to_string()).execute(&mut **tx).await.map_err(storage)?;
    append_event_tx(
        tx,
        "system",
        "task-revision",
        "task",
        task_id,
        "task.revision_applied",
        json!({"revision_id":revision_id,"version":version,"context_revision_id":new_context}),
        None,
    )
    .await?;
    Ok(())
}

impl Store {
    pub async fn migrate_task_revision_baselines(&self) -> Result<(), DomainError> {
        let ids:Vec<String>=sqlx::query_scalar("SELECT id FROM tasks WHERE NOT EXISTS(SELECT 1 FROM task_revisions WHERE task_id=tasks.id)")
         .fetch_all(&self.pool).await.map_err(storage)?;
        for id in ids {
            let mut tx = self
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(storage)?;
            baseline_tx(&mut tx, parse_id(id)?).await?;
            tx.commit().await.map_err(storage)?;
        }
        Ok(())
    }
    pub async fn task_revision_history(&self, task_id: Id) -> Result<Value, DomainError> {
        let task = self.get_task(task_id).await?;
        let active_version: i64 = sqlx::query_scalar("SELECT spec_version FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_one(&self.pool)
            .await
            .map_err(storage)?;
        let rows =
            sqlx::query("SELECT * FROM task_revisions WHERE task_id=? ORDER BY version DESC")
                .bind(task_id.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(storage)?;
        let history: Vec<Value> = rows
            .into_iter()
            .map(revision_json)
            .collect::<Result<_, _>>()?;
        Ok(
            json!({"task_id":task_id,"active_version":active_version,"acceptance_version":task.acceptance_version,
         "context_revision_id":task.current_context_revision_id,"revisions":history}),
        )
    }
    pub async fn preview_task_revision(
        &self,
        task_id: Id,
        draft: TaskRevisionDraft,
    ) -> Result<Value, DomainError> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let (before, version, state, context_id, _) = spec_conn(&mut tx, task_id).await?;
        if version != draft.expected_version {
            return Err(DomainError::Conflict("stale task spec version".into()));
        }
        if state == "done" || state == "cancelled" {
            return Err(DomainError::Conflict(
                "completed/cancelled tasks require rework".into(),
            ));
        }
        let after = patched(&before, &draft)?;
        Ok(
            json!({"active_version":version,"status_if_submitted":if state=="in_progress"||state=="review"{"pending_ack"}else{"applied"},
          "context_revision_id":context_id,"before":before,"after":after,"diff":diff(&before,&after)}),
        )
    }
    pub async fn propose_task_revision(
        &self,
        task_id: Id,
        actor: &str,
        draft: TaskRevisionDraft,
    ) -> Result<Value, DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let (before, current_version, state, context_id, owner) =
            spec_conn(&mut tx, task_id).await?;
        if actor != owner && !actor.starts_with("human:") && !actor.starts_with("operator:") {
            return Err(DomainError::Conflict(
                "only publisher or authorized Human operator can revise Task".into(),
            ));
        }
        if state == "done" || state == "cancelled" {
            return Err(DomainError::Conflict(
                "completed/cancelled Task must use rework".into(),
            ));
        }
        if current_version != draft.expected_version {
            return Err(DomainError::Conflict(
                "stale Task revision version (CAS)".into(),
            ));
        }
        let active: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task_revisions WHERE task_id=? AND status='pending_ack')",
        )
        .bind(task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if active {
            return Err(DomainError::Conflict(
                "task has pending revision; resolve it before proposing another".into(),
            ));
        }
        let after = patched(&before, &draft)?;
        baseline_tx(&mut tx, task_id).await?;
        let next_version: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(version),0)+1 FROM task_revisions WHERE task_id=?",
        )
        .bind(task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        let in_flight: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE task_id=? AND status IN ('running','paused'))",
        )
        .bind(task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        let pending = in_flight || matches!(state.as_str(), "in_progress" | "review");
        let id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO task_revisions(id,task_id,version,base_version,status,author_actor_id,reason,before_json,after_json,context_revision_before,created_at) VALUES(?,?,?,?,'pending_ack',?,?,?,?,?,?)")
          .bind(id.to_string()).bind(task_id.to_string()).bind(next_version).bind(current_version)
          .bind(actor).bind(draft.reason.trim()).bind(serde_json::to_string(&before).map_err(storage)?)
          .bind(serde_json::to_string(&after).map_err(storage)?).bind(context_id)
          .bind(&now).execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(&mut tx,"system","task-revision","task",task_id,"task.revision_proposed",
          json!({"revision_id":id,"version":next_version,"base_version":current_version,"reason":draft.reason,"diff":diff(&before,&after),"status":if pending{"pending_ack"}else{"applied"}}),None).await?;
        if pending {
            // Durably notify the last executor (active or temporarily offline); the next executor
            // always sees the pending contract in task_revision_history and is blocked until ack.
            let recipient:Option<String>=sqlx::query_scalar("SELECT agent_instance_id FROM assignments WHERE task_id=? AND role='executor' ORDER BY CASE status WHEN 'active' THEN 0 ELSE 1 END,acquired_at DESC LIMIT 1")
             .bind(task_id.to_string()).fetch_optional(&mut *tx).await.map_err(storage)?;
            if let Some(recipient) = recipient {
                let recipient = parse_id(recipient)?;
                let thread = self
                    .ensure_task_agent_thread_tx(&mut tx, task_id, recipient)
                    .await?;
                let message_id = Uuid::new_v4();
                let body = format!(
                    "Task revision v{next_version} pending acknowledgment. Read task_revision_history for diff and task_revision_ack with updated plan. Reason: {}",
                    draft.reason
                );
                sqlx::query("INSERT INTO messages(id,thread_id,created_by,author_type,body,created_at,message_type,recipient_agent_instance_id,requires_response,status,client_message_id,recalled_at) VALUES(?,?,NULL,'system',?,?,'task_revision',?,1,'queued',NULL,NULL)")
              .bind(message_id.to_string()).bind(thread.to_string()).bind(&body).bind(&now).bind(recipient.to_string())
              .execute(&mut *tx).await.map_err(storage)?;
                enqueue_agent_delivery_tx(&mut tx,recipient,Some(task_id),"task_message",message_id,
              json!({"thread_id":thread,"message_id":message_id,"body":body,"revision_id":id,"version":next_version})).await?;
            }
        } else {
            apply_tx(&mut tx, task_id, id, None).await?;
        }
        tx.commit().await.map_err(storage)?;
        self.task_revision_history(task_id).await
    }
    pub async fn ack_task_revision(
        &self,
        task_id: Id,
        agent_id: Id,
        ack: TaskRevisionAck,
    ) -> Result<Value, DomainError> {
        if ack.impact.trim().is_empty()
            || ack.updated_plan.is_empty()
            || ack.updated_plan.iter().any(|x| x.trim().is_empty())
        {
            return Err(DomainError::InvalidInput(
                "impact and non-empty updated_plan required".into(),
            ));
        }
        let run_id = parse_id(ack.run_id)?;
        let rev_id = parse_id(ack.revision_id)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row = sqlx::query("SELECT status,task_id FROM task_revisions WHERE id=?")
            .bind(rev_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound("revision".into()))?;
        let status: String = row.try_get("status").map_err(storage)?;
        let owner: String = row.try_get("task_id").map_err(storage)?;
        if owner != task_id.to_string() || status != "pending_ack" {
            return Err(DomainError::Conflict(
                "revision is no longer pending for this task".into(),
            ));
        }
        let run:Option<String>=sqlx::query_scalar("SELECT r.id FROM runs r JOIN assignments a ON a.id=r.assignment_id WHERE r.id=? AND r.task_id=? AND r.agent_instance_id=? AND r.status IN ('running','paused') AND a.role='executor' AND a.status='active' AND a.expires_at>? AND a.phase='implementing'")
           .bind(run_id.to_string()).bind(task_id.to_string()).bind(agent_id.to_string())
           .bind(Utc::now().to_rfc3339()).fetch_optional(&mut *tx).await.map_err(storage)?;
        if run.is_none() {
            return Err(DomainError::Conflict(
                "only active implementing Task executor can acknowledge revision".into(),
            ));
        }
        apply_tx(
            &mut tx,
            task_id,
            rev_id,
            Some((run_id, agent_id, &ack.impact, &ack.updated_plan)),
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.task_revision_history(task_id).await
    }
    pub async fn reject_task_revision(
        &self,
        task_id: Id,
        actor: &str,
        revision_id: Id,
        reason: &str,
    ) -> Result<Value, DomainError> {
        if reason.trim().is_empty() {
            return Err(DomainError::InvalidInput("reject reason required".into()));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let (_, _, state, _, owner) = spec_conn(&mut tx, task_id).await?;
        if state == "done" || state == "cancelled" {
            return Err(DomainError::Conflict("Task is terminal".into()));
        }
        let owns = owner == actor;
        let is_executor = if let Some(agent) = actor.strip_prefix("agent:") {
            sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM assignments WHERE task_id=? AND agent_instance_id=? AND role='executor' AND status='active' AND expires_at>?)")
             .bind(task_id.to_string()).bind(agent).bind(Utc::now().to_rfc3339())
             .fetch_one(&mut *tx).await.map_err(storage)?
        } else {
            false
        };
        if !owns && !is_executor && !actor.starts_with("human:") && !actor.starts_with("operator:")
        {
            return Err(DomainError::Conflict(
                "only publisher, current executor, or Human operator may reject revision".into(),
            ));
        }
        let n=sqlx::query("UPDATE task_revisions SET status='rejected',resolved_at=?,ack_impact=? WHERE id=? AND task_id=? AND status='pending_ack'")
         .bind(Utc::now().to_rfc3339()).bind(reason).bind(revision_id.to_string()).bind(task_id.to_string())
         .execute(&mut *tx).await.map_err(storage)?.rows_affected();
        if n != 1 {
            return Err(DomainError::Conflict("revision not pending".into()));
        }
        append_event_tx(
            &mut tx,
            "system",
            "task-revision",
            "task",
            task_id,
            "task.revision_rejected",
            json!({"revision_id":revision_id,"reason":reason,"actor":actor}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.task_revision_history(task_id).await
    }
}
