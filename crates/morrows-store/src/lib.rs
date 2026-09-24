use chrono::{DateTime, Duration, Utc};
use morrows_core::{
    AgentInstance, Assignment, ContextRevision, CreateContextRevision, CreateTask, DomainError,
    Event, Id, Run, Task, TaskState,
};
use serde_json::{Value, json};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{str::FromStr, time::Duration as StdDuration};
use uuid::Uuid;

mod runtime;

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
}

impl Store {
    pub async fn connect(database_url: &str) -> Result<Self, DomainError> {
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(storage)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(StdDuration::from_secs(5));
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(storage)?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(storage)?;
        Ok(Self { pool })
    }

    pub async fn create_task(&self, input: CreateTask) -> Result<Task, DomainError> {
        if input.title.trim().is_empty() {
            return Err(DomainError::InvalidInput(
                "task title cannot be empty".into(),
            ));
        }
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self.pool.begin().await.map_err(storage)?;
        sqlx::query(
            "INSERT INTO tasks(id,project_id,title,description,owner_actor_id,state,priority,created_at,updated_at)
             VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(input.project_id.map(|v| v.to_string()))
        .bind(input.title.trim())
        .bind(&input.description)
        .bind(&input.owner_actor_id)
        .bind(input.state.to_string())
        .bind(input.priority)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&mut *tx).await.map_err(storage)?;
        let (actor_type, actor_id) = input
            .owner_actor_id
            .strip_prefix("agent:")
            .map(|id| ("agent_instance", id))
            .unwrap_or(("human", input.owner_actor_id.as_str()));
        append_event_tx(
            &mut tx,
            actor_type,
            actor_id,
            "task",
            id,
            "task.created",
            json!({"state": input.state, "title": input.title}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_task(id).await
    }

    pub async fn list_tasks(&self) -> Result<Vec<Task>, DomainError> {
        let rows = sqlx::query("SELECT * FROM tasks ORDER BY priority DESC, created_at DESC")
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        rows.into_iter().map(row_to_task).collect()
    }

    pub async fn get_task(&self, id: Id) -> Result<Task, DomainError> {
        let row = sqlx::query("SELECT * FROM tasks WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {id}")))?;
        row_to_task(row)
    }

    pub async fn claim_task(
        &self,
        task_id: Id,
        agent_id: Id,
        role: &str,
        lease_seconds: i64,
    ) -> Result<Assignment, DomainError> {
        self.get_agent(agent_id).await?;
        let now = Utc::now();
        let expires = now + Duration::seconds(lease_seconds.max(30));
        let id = Uuid::new_v4();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        if role.trim().is_empty() {
            return Err(DomainError::InvalidInput("role cannot be empty".into()));
        }
        if role == "executor" {
            let blocked: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task_dependencies d JOIN tasks t ON t.id=d.depends_on_task_id WHERE d.task_id=? AND t.state!='done')").bind(task_id.to_string()).fetch_one(&mut *tx).await.map_err(storage)?;
            if blocked {
                return Err(DomainError::Conflict(
                    "task has unfinished dependencies".into(),
                ));
            }
        }
        let cleanup_blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE task_id=? AND status IN ('interrupted','cleanup_pending','cancelling'))",
        )
        .bind(task_id.to_string()).fetch_one(&mut *tx).await.map_err(storage)?;
        if cleanup_blocked {
            return Err(DomainError::Conflict(
                "Task has an interrupted or cleaning Run".into(),
            ));
        }

        // BEGIN IMMEDIATE keeps prerequisite checks and the role claim atomic.
        // The unique active-role index remains the final ownership constraint.
        let result = sqlx::query(
            "INSERT INTO assignments(id,task_id,role,agent_instance_id,status,acquired_at,expires_at,renewed_at)
             SELECT ?, id, ?, ?, 'active', ?, ?, ? FROM tasks
             WHERE id=? AND state NOT IN ('done','cancelled')"
        )
            .bind(id.to_string())
            .bind(role)
            .bind(agent_id.to_string())
            .bind(now.to_rfc3339())
            .bind(expires.to_rfc3339())
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut *tx).await;

        match result {
            Ok(r) if r.rows_affected() == 0 => {
                let state: Option<String> =
                    sqlx::query_scalar("SELECT state FROM tasks WHERE id=?")
                        .bind(task_id.to_string())
                        .fetch_optional(&mut *tx)
                        .await
                        .map_err(storage)?;
                return match state {
                    None => Err(DomainError::NotFound(format!("task {task_id}"))),
                    Some(state) => Err(DomainError::Conflict(format!("task is {state}"))),
                };
            }
            Ok(_) => {}
            Err(e) => {
                if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("2067") {
                    return Err(DomainError::Conflict(format!(
                        "task role '{role}' already has an active assignment"
                    )));
                }
                return Err(storage(e));
            }
        }

        sqlx::query("UPDATE tasks SET state=CASE WHEN state='ready' THEN 'in_progress' ELSE state END, updated_at=? WHERE id=?")
            .bind(now.to_rfc3339()).bind(task_id.to_string()).execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task",
            task_id,
            "assignment.claimed",
            json!({"assignment_id":id,"role":role,"expires_at":expires}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(Assignment {
            id,
            task_id,
            role: role.to_owned(),
            agent_instance_id: agent_id,
            status: "active".into(),
            acquired_at: now,
            expires_at: expires,
            renewed_at: now,
        })
    }

    pub async fn task_assignments(&self, task_id: Id) -> Result<Vec<Assignment>, DomainError> {
        let rows =
            sqlx::query("SELECT * FROM assignments WHERE task_id=? ORDER BY acquired_at DESC")
                .bind(task_id.to_string())
                .fetch_all(&self.pool)
                .await
                .map_err(storage)?;
        rows.into_iter().map(row_to_assignment).collect()
    }

    pub async fn task_runs(&self, task_id: Id) -> Result<Vec<Run>, DomainError> {
        let rows = sqlx::query(
            "SELECT runs.*, rcr.context_revision_id
             FROM runs
             LEFT JOIN run_context_revisions rcr ON rcr.run_id=runs.id
             WHERE runs.task_id=?
             ORDER BY runs.started_at DESC",
        )
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_run).collect()
    }

    pub async fn get_assignment(&self, id: Id) -> Result<Assignment, DomainError> {
        let row = sqlx::query("SELECT * FROM assignments WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("assignment {id}")))?;
        row_to_assignment(row)
    }

    pub async fn renew_assignment(
        &self,
        id: Id,
        actor_agent_id: Id,
        lease_seconds: i64,
    ) -> Result<Assignment, DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let assignment = row_to_assignment(
            sqlx::query("SELECT * FROM assignments WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound("assignment".into()))?,
        )?;
        if assignment.agent_instance_id != actor_agent_id {
            return Err(DomainError::Conflict(
                "assignment belongs to another agent instance".into(),
            ));
        }
        if assignment.status != "active" {
            return Err(DomainError::Conflict(format!(
                "assignment is {}",
                assignment.status
            )));
        }
        let now = Utc::now();
        if assignment.expires_at <= now {
            return Err(DomainError::Conflict("assignment lease has expired".into()));
        }
        let expires = now + Duration::seconds(lease_seconds.max(30));
        sqlx::query(
            "UPDATE assignments SET renewed_at=?,expires_at=? WHERE id=? AND status='active'",
        )
        .bind(now.to_rfc3339())
        .bind(expires.to_rfc3339())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor_agent_id.to_string(),
            "task",
            assignment.task_id,
            "assignment.renewed",
            json!({"assignment_id":id,"expires_at":expires}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_assignment(id).await
    }

    pub async fn expire_stale_assignments(&self) -> Result<u64, DomainError> {
        let now = Utc::now();
        let rows=sqlx::query("SELECT id,task_id,agent_instance_id FROM assignments WHERE status='active' AND expires_at<=?
            AND NOT EXISTS(SELECT 1 FROM runs WHERE runs.assignment_id=assignments.id AND runs.status IN ('interrupted','cleanup_pending','cancelling'))")
            .bind(now.to_rfc3339()).fetch_all(&self.pool).await.map_err(storage)?;
        if rows.is_empty() {
            return Ok(0);
        }
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let mut count = 0u64;
        for row in rows {
            let id = parse_id(row.try_get("id").map_err(storage)?)?;
            let task_id = parse_id(row.try_get("task_id").map_err(storage)?)?;
            let agent_id = parse_id(row.try_get("agent_instance_id").map_err(storage)?)?;
            let result=sqlx::query("UPDATE assignments SET status='expired',released_at=?,release_reason='lease_expired' WHERE id=? AND status='active'")
                .bind(now.to_rfc3339()).bind(id.to_string()).execute(&mut *tx).await.map_err(storage)?;
            if result.rows_affected() == 1 {
                count += 1;
                append_event_tx(
                    &mut tx,
                    "system",
                    "lease-manager",
                    "task",
                    task_id,
                    "assignment.expired",
                    json!({"assignment_id":id,"agent_instance_id":agent_id}),
                    None,
                )
                .await?;
            }
        }
        tx.commit().await.map_err(storage)?;
        Ok(count)
    }

    pub async fn start_run(
        &self,
        assignment_id: Id,
        actor_agent_id: Id,
        external_session_ref: Option<String>,
    ) -> Result<Run, DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row = sqlx::query(
            "SELECT task_id,agent_instance_id,status,expires_at FROM assignments WHERE id=?",
        )
        .bind(assignment_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("assignment {assignment_id}")))?;
        let status: String = row.try_get("status").map_err(storage)?;
        let expires = parse_dt(row.try_get("expires_at").map_err(storage)?)?;
        if status != "active" || expires <= Utc::now() {
            return Err(DomainError::Conflict("assignment is not active".into()));
        }
        let task_id = Uuid::parse_str(
            row.try_get::<String, _>("task_id")
                .map_err(storage)?
                .as_str(),
        )
        .map_err(storage)?;
        let agent_id = Uuid::parse_str(
            row.try_get::<String, _>("agent_instance_id")
                .map_err(storage)?
                .as_str(),
        )
        .map_err(storage)?;
        if agent_id != actor_agent_id {
            return Err(DomainError::Conflict(
                "assignment belongs to another agent instance".into(),
            ));
        }
        let id = Uuid::new_v4();
        let now = Utc::now();
        let current_ctx_rev: Option<String> = sqlx::query_scalar(
            "SELECT current_context_revision_id FROM tasks WHERE id=?",
        )
        .bind(task_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .flatten();

        sqlx::query("INSERT INTO runs(id,task_id,assignment_id,agent_instance_id,external_session_ref,status,started_at) VALUES(?,?,?,?,?,'running',?)")
            .bind(id.to_string()).bind(task_id.to_string()).bind(assignment_id.to_string()).bind(agent_id.to_string())
            .bind(&external_session_ref).bind(now.to_rfc3339()).execute(&mut *tx).await.map_err(storage)?;

        if let Some(ctx_id) = current_ctx_rev {
            sqlx::query(
                "INSERT INTO run_context_revisions(run_id,context_revision_id,pinned_at) VALUES(?,?,?)",
            )
            .bind(id.to_string())
            .bind(ctx_id)
            .bind(now.to_rfc3339())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        }

        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "run",
            id,
            "run.started",
            json!({"task_id":task_id,"assignment_id":assignment_id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_run(id).await
    }

    pub async fn get_run(&self, id: Id) -> Result<Run, DomainError> {
        let row = sqlx::query(
            "SELECT runs.*, rcr.context_revision_id
             FROM runs
             LEFT JOIN run_context_revisions rcr ON rcr.run_id=runs.id
             WHERE runs.id=?",
        )
        .bind(id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("run {id}")))?;
        row_to_run(row)
    }

    pub async fn checkpoint_run(
        &self,
        id: Id,
        actor_agent_id: Id,
        checkpoint: Value,
    ) -> Result<Run, DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound("run".into()))?,
        )?;
        if run.agent_instance_id != actor_agent_id {
            return Err(DomainError::Conflict(
                "run belongs to another agent instance".into(),
            ));
        }
        if run.status != "running" && run.status != "paused" {
            return Err(DomainError::Conflict(format!("run is {}", run.status)));
        }
        let active: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assignments WHERE id=? AND status='active' AND expires_at>?)").bind(run.assignment_id.to_string()).bind(Utc::now().to_rfc3339()).fetch_one(&mut *tx).await.map_err(storage)?;
        if !active {
            return Err(DomainError::Conflict("assignment is not active".into()));
        }
        sqlx::query("UPDATE runs SET checkpoint_json=? WHERE id=?")
            .bind(checkpoint.to_string())
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &run.agent_instance_id.to_string(),
            "run",
            id,
            "run.checkpointed",
            json!({"task_id":run.task_id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_run(id).await
    }

    pub async fn complete_run(
        &self,
        id: Id,
        actor_agent_id: Id,
        result: Value,
    ) -> Result<Run, DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound("run".into()))?,
        )?;
        if run.agent_instance_id != actor_agent_id {
            return Err(DomainError::Conflict(
                "run belongs to another agent instance".into(),
            ));
        }
        if run.status != "running" && run.status != "paused" {
            return Err(DomainError::Conflict(format!("run is {}", run.status)));
        }
        let active: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM assignments WHERE id=? AND status='active' AND expires_at>?)").bind(run.assignment_id.to_string()).bind(Utc::now().to_rfc3339()).fetch_one(&mut *tx).await.map_err(storage)?;
        if !active {
            return Err(DomainError::Conflict("assignment is not active".into()));
        }
        let now = Utc::now();
        sqlx::query("UPDATE runs SET status='completed', stop_reason='normal', result_json=?, ended_at=? WHERE id=?")
            .bind(result.to_string()).bind(now.to_rfc3339()).bind(id.to_string()).execute(&mut *tx).await.map_err(storage)?;
        let assignment_role: String = sqlx::query_scalar("SELECT role FROM assignments WHERE id=?")
            .bind(run.assignment_id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage)?;
        sqlx::query("UPDATE assignments SET status='completed', released_at=?, release_reason='run_completed' WHERE id=?")
            .bind(now.to_rfc3339()).bind(run.assignment_id.to_string()).execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &run.agent_instance_id.to_string(),
            "run",
            id,
            "run.completed",
            json!({"task_id":run.task_id,"role":assignment_role}),
            None,
        )
        .await?;
        if assignment_role == "executor" {
            let blocked: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task_dependencies d JOIN tasks t ON t.id=d.depends_on_task_id WHERE d.task_id=? AND t.state!='done')").bind(run.task_id.to_string()).fetch_one(&mut *tx).await.map_err(storage)?;
            if blocked {
                return Err(DomainError::Conflict(
                    "task has unfinished dependencies".into(),
                ));
            }
            sqlx::query("UPDATE tasks SET state='done', updated_at=? WHERE id=?")
                .bind(now.to_rfc3339())
                .bind(run.task_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            append_event_tx(
                &mut tx,
                "agent_instance",
                &run.agent_instance_id.to_string(),
                "task",
                run.task_id,
                "task.state_changed",
                json!({"state":"done"}),
                None,
            )
            .await?;
        }
        tx.commit().await.map_err(storage)?;
        self.get_run(id).await
    }

    pub async fn create_context_revision(
        &self,
        task_id: Id,
        input: CreateContextRevision,
    ) -> Result<ContextRevision, DomainError> {
        self.get_task(task_id).await?;
        let now = Utc::now();
        let id = Uuid::new_v4();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let latest=sqlx::query("SELECT id,version FROM context_revisions WHERE task_id=? ORDER BY version DESC LIMIT 1")
            .bind(task_id.to_string()).fetch_optional(&mut *tx).await.map_err(storage)?;
        let (parent, version) = if let Some(row) = latest {
            (
                Some(row.try_get::<String, _>("id").map_err(storage)?),
                row.try_get::<i64, _>("version").map_err(storage)? + 1,
            )
        } else {
            (None, 1)
        };
        sqlx::query("INSERT INTO context_revisions(id,task_id,version,parent_revision_id,goal,background,constraints_json,current_summary,created_by_actor_id,created_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
            .bind(id.to_string()).bind(task_id.to_string()).bind(version).bind(&parent).bind(&input.goal).bind(&input.background)
            .bind(input.constraints.to_string()).bind(&input.current_summary).bind(&input.created_by_actor_id).bind(now.to_rfc3339())
            .execute(&mut *tx).await.map_err(storage)?;
        sqlx::query("UPDATE tasks SET current_context_revision_id=?,updated_at=? WHERE id=?")
            .bind(id.to_string())
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        let actor_type = if input.created_by_actor_id.starts_with("agent:") {
            "agent_instance"
        } else if input.created_by_actor_id.starts_with("system:") {
            "system"
        } else {
            "human"
        };
        append_event_tx(
            &mut tx,
            actor_type,
            &input.created_by_actor_id,
            "task",
            task_id,
            "context.revised",
            json!({"context_revision_id":id,"version":version}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_context_revision(id).await
    }

    pub async fn get_current_context(&self, task_id: Id) -> Result<ContextRevision, DomainError> {
        let task = self.get_task(task_id).await?;
        let id = task
            .current_context_revision_id
            .ok_or_else(|| DomainError::NotFound(format!("context for task {task_id}")))?;
        self.get_context_revision(id).await
    }

    pub async fn get_context_revision(&self, id: Id) -> Result<ContextRevision, DomainError> {
        let row = sqlx::query("SELECT * FROM context_revisions WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("context revision {id}")))?;
        Ok(ContextRevision {
            id,
            task_id: parse_id(row.try_get("task_id").map_err(storage)?)?,
            version: row.try_get("version").map_err(storage)?,
            parent_revision_id: parse_opt_id(row.try_get("parent_revision_id").map_err(storage)?)?,
            goal: row.try_get("goal").map_err(storage)?,
            background: row.try_get("background").map_err(storage)?,
            constraints: parse_json(row.try_get("constraints_json").map_err(storage)?)?,
            current_summary: row.try_get("current_summary").map_err(storage)?,
            created_by_actor_id: row.try_get("created_by_actor_id").map_err(storage)?,
            created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        })
    }

    pub async fn task_events(&self, task_id: Id) -> Result<Vec<Event>, DomainError> {
        let rows = sqlx::query(
            "SELECT * FROM events WHERE (entity_type='task' AND entity_id=?)
             OR (json_extract(payload_json,'$.task_id')=?) ORDER BY created_at,id",
        )
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_event).collect()
    }
}

fn row_to_task(row: sqlx::sqlite::SqliteRow) -> Result<Task, DomainError> {
    let state: String = row.try_get("state").map_err(storage)?;
    Ok(Task {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        project_id: parse_opt_id(row.try_get("project_id").map_err(storage)?)?,
        title: row.try_get("title").map_err(storage)?,
        description: row.try_get("description").map_err(storage)?,
        owner_actor_id: row.try_get("owner_actor_id").map_err(storage)?,
        state: TaskState::from_str(&state)?,
        priority: row.try_get("priority").map_err(storage)?,
        current_context_revision_id: parse_opt_id(
            row.try_get("current_context_revision_id")
                .map_err(storage)?,
        )?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}

fn row_to_assignment(row: sqlx::sqlite::SqliteRow) -> Result<Assignment, DomainError> {
    Ok(Assignment {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        task_id: parse_id(row.try_get("task_id").map_err(storage)?)?,
        role: row.try_get("role").map_err(storage)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        status: row.try_get("status").map_err(storage)?,
        acquired_at: parse_dt(row.try_get("acquired_at").map_err(storage)?)?,
        expires_at: parse_dt(row.try_get("expires_at").map_err(storage)?)?,
        renewed_at: parse_dt(row.try_get("renewed_at").map_err(storage)?)?,
    })
}

fn row_to_run(row: sqlx::sqlite::SqliteRow) -> Result<Run, DomainError> {
    let status: String = row.try_get("status").map_err(storage)?;
    let stop_reason: Option<String> = row.try_get("stop_reason").map_err(storage)?;
    Ok(Run {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        task_id: parse_id(row.try_get("task_id").map_err(storage)?)?,
        assignment_id: parse_id(row.try_get("assignment_id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        external_session_ref: row.try_get("external_session_ref").map_err(storage)?,
        failure_reason: if status == "failed" {
            stop_reason.clone()
        } else {
            None
        },
        status,
        stop_reason,
        checkpoint: parse_opt_json(row.try_get("checkpoint_json").map_err(storage)?)?,
        result: parse_opt_json(row.try_get("result_json").map_err(storage)?)?,
        started_at: parse_dt(row.try_get("started_at").map_err(storage)?)?,
        ended_at: parse_opt_dt(row.try_get("ended_at").map_err(storage)?)?,
        context_revision_id: parse_opt_id(row.try_get("context_revision_id").ok().flatten())?,
    })
}

fn row_to_event(row: sqlx::sqlite::SqliteRow) -> Result<Event, DomainError> {
    Ok(Event {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        actor_type: row.try_get("actor_type").map_err(storage)?,
        actor_id: row.try_get("actor_id").map_err(storage)?,
        entity_type: row.try_get("entity_type").map_err(storage)?,
        entity_id: parse_id(row.try_get("entity_id").map_err(storage)?)?,
        event_type: row.try_get("event_type").map_err(storage)?,
        correlation_id: row.try_get("correlation_id").map_err(storage)?,
        payload: parse_json(row.try_get("payload_json").map_err(storage)?)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}

async fn append_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    actor_type: &str,
    actor_id: &str,
    entity_type: &str,
    entity_id: Id,
    event_type: &str,
    payload: Value,
    correlation_id: Option<&str>,
) -> Result<(), DomainError> {
    sqlx::query("INSERT INTO events(id,actor_type,actor_id,entity_type,entity_id,event_type,correlation_id,payload_json,created_at) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(Uuid::new_v4().to_string()).bind(actor_type).bind(actor_id).bind(entity_type).bind(entity_id.to_string())
        .bind(event_type).bind(correlation_id).bind(payload.to_string()).bind(Utc::now().to_rfc3339())
        .execute(&mut **tx).await.map_err(storage)?;
    Ok(())
}

fn parse_id(v: String) -> Result<Uuid, DomainError> {
    Uuid::parse_str(&v).map_err(storage)
}
fn parse_opt_id(v: Option<String>) -> Result<Option<Uuid>, DomainError> {
    v.map(|x| parse_id(x)).transpose()
}
fn parse_dt(v: String) -> Result<DateTime<Utc>, DomainError> {
    DateTime::parse_from_rfc3339(&v)
        .map(|x| x.with_timezone(&Utc))
        .map_err(storage)
}
fn parse_opt_dt(v: Option<String>) -> Result<Option<DateTime<Utc>>, DomainError> {
    v.map(parse_dt).transpose()
}
fn parse_json(v: String) -> Result<Value, DomainError> {
    serde_json::from_str(&v).map_err(storage)
}
fn parse_opt_json(v: Option<String>) -> Result<Option<Value>, DomainError> {
    v.map(parse_json).transpose()
}
fn storage<E: std::fmt::Display>(e: E) -> DomainError {
    DomainError::Storage(e.to_string())
}

mod collaboration;

mod fleet;

mod dispatch;

mod launch;

mod conversation;

mod context_package;

mod delivery;

mod auth;
