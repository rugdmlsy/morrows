use chrono::{DateTime, Duration, Utc};
use morrows_core::{
    AgentInstance, Assignment, AssignmentMode, ContextRevision, CreateContextRevision,
    CreateProject, CreateTask, DomainError, Event, Id, Project, Run, Task, TaskClaim,
    TaskManagementSummary, TaskState, UpdateContextRevision,
};
use serde_json::{Value, json};
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use std::{str::FromStr, time::Duration as StdDuration};
use uuid::Uuid;

mod continuation;
mod discovery;
mod git_memory;
mod identity;
mod job_wait;
mod milestone;
mod runtime;
mod task_rework;

enum ContextRevisionWrite {
    Replace(CreateContextRevision),
    Patch(UpdateContextRevision),
}

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
    git_memory_path: std::path::PathBuf,
    #[cfg(test)]
    fail_after_memory_cas: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl Store {
    pub async fn connect(database_url: &str) -> Result<Self, DomainError> {
        let options = SqliteConnectOptions::from_str(database_url)
            .map_err(storage)?
            .create_if_missing(true)
            .foreign_keys(true)
            .busy_timeout(StdDuration::from_secs(5));
        let git_memory_path = if database_url == "sqlite::memory:" {
            std::env::temp_dir().join(format!("morrows-knowledge-{}.git", Uuid::new_v4()))
        } else {
            options.get_filename().with_extension("knowledge.git")
        };
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .map_err(storage)?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(storage)?;
        let store = Self {
            pool,
            git_memory_path,
            #[cfg(test)]
            fail_after_memory_cas: Default::default(),
        };
        store.reconcile_git_memory().await?;
        Ok(store)
    }

    pub async fn create_project(&self, input: CreateProject) -> Result<Project, DomainError> {
        let name = input.name.trim();
        if name.is_empty() {
            return Err(DomainError::InvalidInput(
                "project name cannot be empty".into(),
            ));
        }
        if name.chars().count() > 120 {
            return Err(DomainError::InvalidInput(
                "project name cannot exceed 120 characters".into(),
            ));
        }
        let id = Uuid::new_v4();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO projects(id,name,description,status,created_at,updated_at)
             VALUES(?,?,?,'active',?,?)",
        )
        .bind(id.to_string())
        .bind(name)
        .bind(&input.description)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        self.get_project(id).await
    }

    pub async fn list_projects(&self) -> Result<Vec<Project>, DomainError> {
        let rows = sqlx::query(
            "SELECT * FROM projects ORDER BY
             CASE status WHEN 'active' THEN 0 ELSE 1 END,
             updated_at DESC, name COLLATE NOCASE ASC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_project).collect()
    }

    pub async fn get_project(&self, id: Id) -> Result<Project, DomainError> {
        let row = sqlx::query("SELECT * FROM projects WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("project {id}")))?;
        let mut project = row_to_project(row)?;
        project.memory_head = self.project_memory_head(id).await?;
        Ok(project)
    }

    pub async fn set_task_project(
        &self,
        task_id: Id,
        project_id: Option<Id>,
    ) -> Result<Task, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let task_row = sqlx::query("SELECT project_id FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?;
        let previous_project_id: Option<String> =
            task_row.try_get("project_id").map_err(storage)?;
        if let Some(project_id) = project_id {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=?)")
                    .bind(project_id.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage)?;
            if !exists {
                return Err(DomainError::NotFound(format!("project {project_id}")));
            }
        }
        sqlx::query("UPDATE tasks SET project_id=?, updated_at=? WHERE id=?")
            .bind(project_id.map(|value| value.to_string()))
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "operator",
            "control-plane",
            "task",
            task_id,
            "task.project_changed",
            json!({
                "previous_project_id": previous_project_id,
                "project_id": project_id,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_task(task_id).await
    }

    pub async fn set_task_project_as_agent(
        &self,
        task_id: Id,
        project_id: Option<Id>,
        agent_id: Id,
    ) -> Result<Task, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let task = memory::task_writer_conn(&mut tx, task_id, agent_id).await?;
        if task.owner_actor_id != format!("agent:{agent_id}") {
            return Err(DomainError::Conflict(
                "only the agent that published the task may change its project".into(),
            ));
        }
        if task.project_id == project_id {
            tx.commit().await.map_err(storage)?;
            return Ok(task);
        }
        let active_implementation: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM assignments WHERE task_id=? AND status='active' AND role='executor' AND phase='implementing')",
        )
        .bind(task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if active_implementation {
            return Err(DomainError::Conflict(
                "task project cannot change while an executor is implementing it".into(),
            ));
        }
        let published_project_memory: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM memory_entries WHERE task_id=? AND scope_type='project')",
        )
        .bind(task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if published_project_memory {
            return Err(DomainError::Conflict(
                "task project cannot change after project memory has been published from the task"
                    .into(),
            ));
        }
        if let Some(project_id) = project_id {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=?)")
                    .bind(project_id.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage)?;
            if !exists {
                return Err(DomainError::NotFound(format!("project {project_id}")));
            }
        }
        sqlx::query("UPDATE tasks SET project_id=?,updated_at=? WHERE id=?")
            .bind(project_id.map(|id| id.to_string()))
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;

        // Project Memory is part of mandatory executor intake. Moving the task
        // invalidates any pre-implementation receipts/approval tied to the old
        // project, so active executors must read/interview again.
        sqlx::query(
            "UPDATE assignments SET phase='context_review' WHERE task_id=? AND status='active' AND role='executor' AND phase!='implementing'",
        )
        .bind(task_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        sqlx::query(
            "UPDATE assignment_intakes SET project_id=NULL,project_memory_head=NULL,project_memory_next_offset=NULL,project_memory_complete=0,project_memory_read_at=NULL,context_package_id=NULL,context_revision_id=NULL,context_package_read_at=NULL,understanding='',constraints_json='{}',plan_json='[]',questions_json='[]',unresolved_questions_json='[]',interview_status='not_started',conversation_state='not_started',human_response=NULL,approved_by_actor_id=NULL,approved_at=NULL,interview_started_at=NULL,final_summary_message_id=NULL,confirmation_message_id=NULL,converged_at=NULL,updated_at=? WHERE task_id=? AND assignment_id IN (SELECT id FROM assignments WHERE task_id=? AND status='active' AND role='executor' AND phase='context_review')",
        )
        .bind(now.to_rfc3339())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task",
            task_id,
            "task.project_changed",
            json!({"previous_project_id":task.project_id,"project_id":project_id,"reason":"publisher_reassignment"}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_task(task_id).await
    }

    pub async fn withdraw_task_as_agent(
        &self,
        task_id: Id,
        agent_id: Id,
        delete: bool,
    ) -> Result<Option<Task>, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let task = memory::task_writer_conn(&mut tx, task_id, agent_id).await?;
        if task.owner_actor_id != format!("agent:{agent_id}") {
            return Err(DomainError::Conflict(
                "only the agent that published the task may withdraw or delete it".into(),
            ));
        }
        let active_implementation: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM assignments WHERE task_id=? AND status='active' AND role='executor' AND phase='implementing')",
        )
        .bind(task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if active_implementation {
            return Err(DomainError::Conflict(
                "task is already being implemented; stop/cancel the active execution before withdrawing it".into(),
            ));
        }

        if delete {
            let assignment_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM assignments WHERE task_id=?")
                    .bind(task_id.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage)?;
            let session_count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM sessions WHERE task_id=?")
                    .bind(task_id.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage)?;
            if assignment_count != 0
                || session_count != 0
                || task.current_context_revision_id.is_some()
            {
                return Err(DomainError::Conflict(
                    "hard delete is only allowed for an unstarted published task with no assignment, Task Session, or context history; use withdraw instead".into(),
                ));
            }
            let deleted = sqlx::query("DELETE FROM tasks WHERE id=?")
                .bind(task_id.to_string())
                .execute(&mut *tx)
                .await;
            match deleted {
                Ok(result) if result.rows_affected() == 1 => {}
                Ok(_) => return Err(DomainError::NotFound(format!("task {task_id}"))),
                Err(_) => {
                    return Err(DomainError::Conflict(
                        "hard delete refused because the task has dependent records; use withdraw instead".into(),
                    ));
                }
            }
            sqlx::query("DELETE FROM events WHERE entity_type='task' AND entity_id=?")
                .bind(task_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            append_event_tx(
                &mut tx,
                "agent_instance",
                &agent_id.to_string(),
                "task",
                task_id,
                "task.deleted",
                json!({"title":task.title,"project_id":task.project_id}),
                None,
            )
            .await?;
            tx.commit().await.map_err(storage)?;
            return Ok(None);
        }

        if task.state == TaskState::Cancelled {
            tx.commit().await.map_err(storage)?;
            return Ok(Some(task));
        }
        if task.state == TaskState::Done {
            return Err(DomainError::Conflict(
                "completed tasks cannot be withdrawn".into(),
            ));
        }
        sqlx::query("UPDATE runs SET status='cancelled',stop_reason='task_withdrawn',ended_at=? WHERE task_id=? AND status IN ('running','paused','interrupted','cancelling')")
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        sqlx::query("UPDATE assignments SET status='released',released_at=?,release_reason='task_withdrawn' WHERE task_id=? AND status='active'")
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        sqlx::query("UPDATE assignment_requests SET status='withdrawn',resolved_at=?,resolution='task_withdrawn' WHERE task_id=? AND status='pending'")
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        sqlx::query("UPDATE tasks SET state='cancelled',updated_at=? WHERE id=?")
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task",
            task_id,
            "task.withdrawn",
            json!({"previous_state":task.state}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(Some(self.get_task(task_id).await?))
    }

    pub async fn delete_task(&self, task_id: Id) -> Result<(), DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let task = row_to_task(
            sqlx::query("SELECT * FROM tasks WHERE id=?")
                .bind(task_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?,
        )?;
        let has_history: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM assignments WHERE task_id=?)
                OR EXISTS(SELECT 1 FROM sessions WHERE task_id=?)
                OR EXISTS(SELECT 1 FROM context_revisions WHERE task_id=?)
                OR EXISTS(SELECT 1 FROM assignment_requests WHERE task_id=?)
                OR EXISTS(SELECT 1 FROM artifacts WHERE task_id=?)
                OR EXISTS(SELECT 1 FROM decisions WHERE task_id=?)
                OR EXISTS(SELECT 1 FROM message_threads WHERE task_id=?)
                OR EXISTS(SELECT 1 FROM handoffs WHERE task_id=?)
                OR EXISTS(SELECT 1 FROM task_dependencies WHERE task_id=? OR depends_on_task_id=?)
                OR EXISTS(SELECT 1 FROM task_relationships WHERE source_task_id=? OR target_task_id=?)
                OR EXISTS(SELECT 1 FROM run_milestones WHERE task_id=?)",
        )
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .bind(task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if has_history || task.current_context_revision_id.is_some() {
            return Err(DomainError::Conflict(
                "task has execution, collaboration, context, or relationship history and cannot be hard-deleted; cancel/withdraw it instead".into(),
            ));
        }
        sqlx::query("DELETE FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(|_| DomainError::Conflict(
                "task has dependent records and cannot be hard-deleted; cancel/withdraw it instead".into(),
            ))?;
        sqlx::query("DELETE FROM events WHERE entity_type='task' AND entity_id=?")
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "human",
            "webui",
            "task",
            task_id,
            "task.deleted",
            json!({"title":task.title,"project_id":task.project_id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(())
    }

    pub async fn set_task_assignment_mode(
        &self,
        task_id: Id,
        assignment_mode: AssignmentMode,
    ) -> Result<Task, DomainError> {
        self.get_task(task_id).await?;
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        sqlx::query("UPDATE tasks SET assignment_mode=?,updated_at=? WHERE id=?")
            .bind(assignment_mode.to_string())
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "operator",
            "control-plane",
            "task",
            task_id,
            "task.assignment_mode_changed",
            json!({"assignment_mode": assignment_mode}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_task(task_id).await
    }

    /// Operator repair path for legacy work requests that were created without
    /// a Project. The first successful bind wins; retrying the same target is
    /// idempotent, while rebinding an already-bound task is rejected.
    pub async fn bind_unbound_task_project(
        &self,
        task_id: Id,
        project_id: Id,
    ) -> Result<Task, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let task_row = sqlx::query("SELECT project_id FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?;
        let previous_project_id: Option<String> =
            task_row.try_get("project_id").map_err(storage)?;
        let target = project_id.to_string();
        if previous_project_id.as_deref() == Some(target.as_str()) {
            tx.commit().await.map_err(storage)?;
            return self.get_task(task_id).await;
        }
        if let Some(current) = previous_project_id {
            return Err(DomainError::Conflict(format!(
                "task {task_id} is already bound to project {current}; repair binding refuses reassignment"
            )));
        }
        let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=?)")
            .bind(&target)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage)?;
        if !exists {
            return Err(DomainError::NotFound(format!("project {project_id}")));
        }
        sqlx::query(
            "UPDATE tasks SET project_id=?, updated_at=? WHERE id=? AND project_id IS NULL",
        )
        .bind(&target)
        .bind(now.to_rfc3339())
        .bind(task_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "operator",
            "repair-cli",
            "task",
            task_id,
            "task.project_changed",
            json!({
                "previous_project_id": null,
                "project_id": project_id,
                "reason": "legacy_unbound_repair",
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_task(task_id).await
    }

    pub async fn create_task(&self, input: CreateTask) -> Result<Task, DomainError> {
        if input.title.trim().is_empty() {
            return Err(DomainError::InvalidInput(
                "task title cannot be empty".into(),
            ));
        }
        let id = Uuid::new_v4();
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        if let Some(project_id) = input.project_id {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM projects WHERE id=?)")
                    .bind(project_id.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage)?;
            if !exists {
                return Err(DomainError::NotFound(format!("project {project_id}")));
            }
        }
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
            json!({
                "state": input.state,
                "assignment_mode": "open",
                "title": input.title,
                "project_id": input.project_id,
            }),
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

    pub async fn list_task_management(&self) -> Result<Vec<TaskManagementSummary>, DomainError> {
        let rows = sqlx::query(
            "SELECT t.*, p.name AS project_name,
                    a.id AS executor_assignment_id,
                    a.status AS executor_assignment_status,
                    a.agent_instance_id AS executor_agent_instance_id,
                    COALESCE(ai.display_name, ai.name) AS executor_agent_display_name,
                    a.acquired_at AS executor_acquired_at,
                    r.id AS latest_run_id,
                    r.status AS latest_run_status,
                    r.started_at AS latest_run_started_at
             FROM tasks t
             LEFT JOIN projects p ON p.id=t.project_id
             LEFT JOIN assignments a ON a.id=(
                 SELECT aa.id FROM assignments aa
                 WHERE aa.task_id=t.id AND aa.role='executor'
                 ORDER BY CASE WHEN aa.status='active' THEN 0 ELSE 1 END, aa.acquired_at DESC
                 LIMIT 1
             )
             LEFT JOIN agent_instances ai ON ai.id=a.agent_instance_id
             LEFT JOIN runs r ON r.id=(
                 SELECT rr.id FROM runs rr
                 WHERE rr.task_id=t.id
                 ORDER BY rr.started_at DESC
                 LIMIT 1
             )
             ORDER BY t.created_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;

        rows.into_iter()
            .map(|row| {
                let project_name = row.try_get("project_name").map_err(storage)?;
                let executor_assignment_id =
                    parse_opt_id(row.try_get("executor_assignment_id").map_err(storage)?)?;
                let executor_assignment_status =
                    row.try_get("executor_assignment_status").map_err(storage)?;
                let executor_agent_instance_id =
                    parse_opt_id(row.try_get("executor_agent_instance_id").map_err(storage)?)?;
                let executor_agent_display_name = row
                    .try_get("executor_agent_display_name")
                    .map_err(storage)?;
                let executor_acquired_at =
                    parse_opt_dt(row.try_get("executor_acquired_at").map_err(storage)?)?;
                let latest_run_id = parse_opt_id(row.try_get("latest_run_id").map_err(storage)?)?;
                let latest_run_status = row.try_get("latest_run_status").map_err(storage)?;
                let latest_run_started_at =
                    parse_opt_dt(row.try_get("latest_run_started_at").map_err(storage)?)?;
                let task = row_to_task(row)?;

                Ok(TaskManagementSummary {
                    task,
                    project_name,
                    executor_assignment_id,
                    executor_assignment_status,
                    executor_agent_instance_id,
                    executor_agent_display_name,
                    executor_acquired_at,
                    latest_run_id,
                    latest_run_status,
                    latest_run_started_at,
                })
            })
            .collect()
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
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let assignment =
            claim_task_tx(&mut tx, task_id, agent_id, role, lease_seconds, false).await?;
        tx.commit().await.map_err(storage)?;
        Ok(assignment)
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

    pub async fn recover_assignment(
        &self,
        assignment_id: Id,
        run_id: Id,
        actor_agent_id: Id,
        lease_seconds: i64,
        reason: &str,
    ) -> Result<TaskClaim, DomainError> {
        let reason = reason.trim();
        if reason.is_empty() || reason.chars().count() > 1000 {
            return Err(DomainError::InvalidInput(
                "recovery reason must contain 1 to 1000 characters".into(),
            ));
        }

        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;

        let assignment_row = sqlx::query("SELECT * FROM assignments WHERE id=?")
            .bind(assignment_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("assignment {assignment_id}")))?;
        let previous_released_at: Option<String> =
            assignment_row.try_get("released_at").map_err(storage)?;
        let previous_release_reason: Option<String> =
            assignment_row.try_get("release_reason").map_err(storage)?;
        let assignment = row_to_assignment(assignment_row)?;

        if assignment.agent_instance_id != actor_agent_id {
            return Err(DomainError::Conflict(
                "expired assignment belongs to another agent instance".into(),
            ));
        }
        if assignment.role != "executor" {
            return Err(DomainError::Conflict(
                "only executor assignments can be recovered".into(),
            ));
        }

        let lease_expired = assignment.expires_at <= now;
        match assignment.status.as_str() {
            "active" if !lease_expired => {
                return Err(DomainError::Conflict(
                    "assignment lease is still active; use assignment_renew".into(),
                ));
            }
            "active" => {}
            "expired" if previous_release_reason.as_deref() == Some("lease_expired") => {}
            "expired" => {
                return Err(DomainError::Conflict(
                    "assignment was not released by lease expiry and cannot be recovered".into(),
                ));
            }
            status => {
                return Err(DomainError::Conflict(format!(
                    "assignment is {status} and cannot be recovered"
                )));
            }
        }

        let task_state: String = sqlx::query_scalar("SELECT state FROM tasks WHERE id=?")
            .bind(assignment.task_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {}", assignment.task_id)))?;
        if task_state != "in_progress" {
            return Err(DomainError::Conflict(format!(
                "task is {task_state}; expired assignment recovery requires in_progress"
            )));
        }

        let conflicting_executor: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM assignments
                WHERE task_id=? AND role='executor' AND status='active' AND id<>?
            )",
        )
        .bind(assignment.task_id.to_string())
        .bind(assignment.id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if conflicting_executor {
            return Err(DomainError::Conflict(
                "task already has another active executor assignment".into(),
            ));
        }

        let run_row = sqlx::query(
            "SELECT runs.*, rcr.context_revision_id
             FROM runs
             LEFT JOIN run_context_revisions rcr ON rcr.run_id=runs.id
             WHERE runs.id=?",
        )
        .bind(run_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("run {run_id}")))?;
        let run = row_to_run(run_row)?;
        if run.assignment_id != assignment.id
            || run.task_id != assignment.task_id
            || run.agent_instance_id != actor_agent_id
        {
            return Err(DomainError::Conflict(
                "run does not belong to the expired assignment and agent".into(),
            ));
        }
        if !matches!(run.status.as_str(), "running" | "paused") {
            return Err(DomainError::Conflict(format!(
                "run is {}; only running or paused runs can be recovered",
                run.status
            )));
        }

        let new_expires_at = now + Duration::seconds(lease_seconds.max(30));
        let update = sqlx::query(
            "UPDATE assignments
             SET status='active', renewed_at=?, expires_at=?, released_at=NULL, release_reason=NULL
             WHERE id=? AND (status='expired' OR (status='active' AND expires_at<=?))",
        )
        .bind(now.to_rfc3339())
        .bind(new_expires_at.to_rfc3339())
        .bind(assignment.id.to_string())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await;
        match update {
            Ok(result) if result.rows_affected() == 1 => {}
            Ok(_) => {
                return Err(DomainError::Conflict(
                    "assignment changed while recovery was being attempted".into(),
                ));
            }
            Err(error)
                if error.as_database_error().and_then(|d| d.code()).as_deref() == Some("2067") =>
            {
                return Err(DomainError::Conflict(
                    "task already has another active executor assignment".into(),
                ));
            }
            Err(error) => return Err(storage(error)),
        }

        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor_agent_id.to_string(),
            "task",
            assignment.task_id,
            "assignment.recovered",
            json!({
                "assignment_id": assignment.id,
                "run_id": run.id,
                "agent_instance_id": actor_agent_id,
                "previous_status": assignment.status,
                "previous_expires_at": assignment.expires_at,
                "previous_released_at": previous_released_at,
                "previous_release_reason": previous_release_reason,
                "new_expires_at": new_expires_at,
                "reason": reason,
            }),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;

        Ok(TaskClaim {
            assignment: self.get_assignment(assignment.id).await?,
            run: self.get_run(run.id).await?,
        })
    }

    pub async fn expire_stale_assignments(&self) -> Result<u64, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let rows=sqlx::query("SELECT id,task_id,agent_instance_id FROM assignments WHERE status='active' AND expires_at<=?
            AND NOT EXISTS(SELECT 1 FROM runs WHERE runs.assignment_id=assignments.id AND runs.status IN ('interrupted','cleanup_pending','cancelling'))")
            .bind(now.to_rfc3339()).fetch_all(&mut *tx).await.map_err(storage)?;
        if rows.is_empty() {
            return Ok(0);
        }
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
        let run = start_run_tx(
            self,
            &mut tx,
            assignment_id,
            actor_agent_id,
            external_session_ref,
            true,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(run)
    }

    /// Employee self-service path for an open task. Assignment acquisition and
    /// Run creation share one IMMEDIATE transaction, so no observable
    /// "assigned but not running" state is produced.
    pub async fn claim_task_for_execution(
        &self,
        task_id: Id,
        agent_id: Id,
        role: &str,
        lease_seconds: i64,
    ) -> Result<TaskClaim, DomainError> {
        self.get_agent(agent_id).await?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row = sqlx::query("SELECT state,assignment_mode FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?;
        let state: String = row.try_get("state").map_err(storage)?;
        let mode: String = row.try_get("assignment_mode").map_err(storage)?;
        if mode != "open" {
            return Err(DomainError::Conflict(match mode.as_str() {
                "approval" => {
                    "task requires assignment approval; use task_request_assignment".into()
                }
                "dispatch" => "task is dispatcher-managed and cannot be self-claimed".into(),
                _ => format!("task assignment mode is {mode}"),
            }));
        }
        if state != "ready" {
            return Err(DomainError::Conflict(format!(
                "task is {state}; only ready tasks can be self-claimed"
            )));
        }

        let assignment =
            claim_task_tx(&mut tx, task_id, agent_id, role, lease_seconds, true).await?;
        let run = start_run_tx(self, &mut tx, assignment.id, agent_id, None, false).await?;

        let now = Utc::now();
        let pending_request_id: Option<String> = sqlx::query_scalar(
            "SELECT id FROM assignment_requests
             WHERE task_id=? AND agent_instance_id=? AND role=? AND status='pending'",
        )
        .bind(task_id.to_string())
        .bind(agent_id.to_string())
        .bind(role)
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?;
        if let Some(request_id) = pending_request_id {
            sqlx::query(
                "UPDATE assignment_requests
                 SET status='approved',assignment_id=?,resolution='auto-approved by open task claim',resolved_at=?
                 WHERE id=? AND status='pending'",
            )
            .bind(assignment.id.to_string())
            .bind(now.to_rfc3339())
            .bind(&request_id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            append_event_tx(
                &mut tx,
                "system",
                "open-task-claim",
                "task",
                task_id,
                "assignment.request_resolved",
                json!({
                    "request_id":request_id,
                    "status":"approved",
                    "assignment_id":assignment.id,
                    "resolution":"auto-approved by open task claim"
                }),
                None,
            )
            .await?;
        }
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task",
            task_id,
            "task.claimed_for_execution",
            json!({"assignment_id":assignment.id,"run_id":run.id,"role":role}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(TaskClaim { assignment, run })
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
        let readiness = completion::completion_check_conn(self, &mut tx, &run, &result).await?;
        if !readiness.ready {
            return Err(DomainError::Conflict(format!(
                "completion blocked: {}",
                readiness.blockers.join("; ")
            )));
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
        self.write_context_revision(task_id, ContextRevisionWrite::Replace(input))
            .await
    }

    pub async fn update_context_revision(
        &self,
        task_id: Id,
        input: UpdateContextRevision,
    ) -> Result<ContextRevision, DomainError> {
        if input.goal.is_none()
            && input.background.is_none()
            && input.current_summary.is_none()
            && input
                .constraints
                .as_ref()
                .is_none_or(serde_json::Map::is_empty)
        {
            return Err(DomainError::InvalidInput(
                "provide at least one context field to update".into(),
            ));
        }
        self.write_context_revision(task_id, ContextRevisionWrite::Patch(input))
            .await
    }

    /// Serialize the read/merge/write under the existing SQLite writer lock.
    /// Two partial updates therefore merge against the actual latest revision;
    /// a supplied base ID rejects stale reasoning before changing any durable state.
    /// The full-replacement control-plane API keeps its existing semantics.
    async fn write_context_revision(
        &self,
        task_id: Id,
        write: ContextRevisionWrite,
    ) -> Result<ContextRevision, DomainError> {
        self.get_task(task_id).await?;
        let now = Utc::now();
        let id = Uuid::new_v4();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let latest = sqlx::query(
            "SELECT * FROM context_revisions WHERE task_id=? ORDER BY version DESC LIMIT 1",
        )
        .bind(task_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?
        .map(row_to_context_revision)
        .transpose()?;
        let parent = latest.as_ref().map(|c| c.id.to_string());
        let version = latest.as_ref().map_or(1, |c| c.version + 1);
        let input = match write {
            ContextRevisionWrite::Replace(input) => input,
            ContextRevisionWrite::Patch(patch) => {
                if patch.expected_context_revision_id.is_some()
                    && patch.expected_context_revision_id != latest.as_ref().map(|c| c.id)
                {
                    return Err(DomainError::Conflict(
                        "context changed; read memory_get and retry with the current revision ID"
                            .into(),
                    ));
                }
                let mut constraints = latest
                    .as_ref()
                    .map_or_else(|| json!({}), |c| c.constraints.clone());
                if let Some(additions) = patch.constraints {
                    let object = constraints.as_object_mut().ok_or_else(|| DomainError::InvalidInput(
                        "existing constraints are not an object; use the control-plane full revision API to replace them".into()
                    ))?;
                    merge_context_constraints(object, additions);
                }
                CreateContextRevision {
                    goal: patch.goal.unwrap_or_else(|| {
                        latest.as_ref().map_or_else(String::new, |c| c.goal.clone())
                    }),
                    background: patch.background.unwrap_or_else(|| {
                        latest
                            .as_ref()
                            .map_or_else(String::new, |c| c.background.clone())
                    }),
                    current_summary: patch.current_summary.unwrap_or_else(|| {
                        latest
                            .as_ref()
                            .map_or_else(String::new, |c| c.current_summary.clone())
                    }),
                    constraints,
                    created_by_actor_id: patch.created_by_actor_id,
                }
            }
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
        row_to_context_revision(row)
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

/// Preserve unmentioned keys at every object level. Explicit non-object values
/// (including arrays and null) replace that key's value, never remove the key.
fn merge_context_constraints(
    current: &mut serde_json::Map<String, Value>,
    additions: serde_json::Map<String, Value>,
) {
    for (key, value) in additions {
        match (current.get_mut(&key), value) {
            (Some(Value::Object(existing)), Value::Object(nested)) => {
                merge_context_constraints(existing, nested)
            }
            (_, value) => {
                current.insert(key, value);
            }
        }
    }
}

fn row_to_context_revision(row: sqlx::sqlite::SqliteRow) -> Result<ContextRevision, DomainError> {
    Ok(ContextRevision {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
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

fn row_to_project(row: sqlx::sqlite::SqliteRow) -> Result<Project, DomainError> {
    Ok(Project {
        memory_head: None,
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        name: row.try_get("name").map_err(storage)?,
        description: row.try_get("description").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}

fn row_to_task(row: sqlx::sqlite::SqliteRow) -> Result<Task, DomainError> {
    let state: String = row.try_get("state").map_err(storage)?;
    let assignment_mode: String = row.try_get("assignment_mode").map_err(storage)?;
    Ok(Task {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        project_id: parse_opt_id(row.try_get("project_id").map_err(storage)?)?,
        title: row.try_get("title").map_err(storage)?,
        description: row.try_get("description").map_err(storage)?,
        owner_actor_id: row.try_get("owner_actor_id").map_err(storage)?,
        state: TaskState::from_str(&state)?,
        assignment_mode: AssignmentMode::from_str(&assignment_mode)?,
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
        phase: row.try_get("phase").map_err(storage)?,
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

mod session;
mod session_runtime;

mod context_package;

mod delivery;

mod auth;

mod operator_auth;

mod completion;
mod memory;

async fn start_run_tx(
    store: &Store,
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    assignment_id: Id,
    actor_agent_id: Id,
    external_session_ref: Option<String>,
    enforce_intake: bool,
) -> Result<Run, DomainError> {
    let row = sqlx::query(
        "SELECT task_id,agent_instance_id,status,expires_at FROM assignments WHERE id=?",
    )
    .bind(assignment_id.to_string())
    .fetch_optional(&mut **tx)
    .await
    .map_err(storage)?
    .ok_or_else(|| DomainError::NotFound(format!("assignment {assignment_id}")))?;
    let status: String = row.try_get("status").map_err(storage)?;
    let expires = parse_dt(row.try_get("expires_at").map_err(storage)?)?;
    if status != "active" || expires <= Utc::now() {
        return Err(DomainError::Conflict("assignment is not active".into()));
    }
    let task_id = parse_id(row.try_get("task_id").map_err(storage)?)?;
    let agent_id = parse_id(row.try_get("agent_instance_id").map_err(storage)?)?;
    if agent_id != actor_agent_id {
        return Err(DomainError::Conflict(
            "assignment belongs to another agent instance".into(),
        ));
    }
    let live: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE assignment_id=?
         AND status IN ('running','paused','interrupted','cleanup_pending','cancelling'))",
    )
    .bind(assignment_id.to_string())
    .fetch_one(&mut **tx)
    .await
    .map_err(storage)?;
    if live {
        return Err(DomainError::Conflict(
            "assignment already has an active run".into(),
        ));
    }
    if enforce_intake {
        let assignment = row_to_assignment(
            sqlx::query("SELECT * FROM assignments WHERE id=?")
                .bind(assignment_id.to_string())
                .fetch_one(&mut **tx)
                .await
                .map_err(storage)?,
        )?;
        intake::enforce_execution_phase_tx(store, tx, &assignment).await?;
    }

    let id = Uuid::new_v4();
    let now = Utc::now();
    let current_ctx_rev: Option<String> =
        sqlx::query_scalar("SELECT current_context_revision_id FROM tasks WHERE id=?")
            .bind(task_id.to_string())
            .fetch_optional(&mut **tx)
            .await
            .map_err(storage)?
            .flatten();

    sqlx::query(
        "INSERT INTO runs(id,task_id,assignment_id,agent_instance_id,external_session_ref,status,started_at)
         VALUES(?,?,?,?,?,'running',?)",
    )
    .bind(id.to_string())
    .bind(task_id.to_string())
    .bind(assignment_id.to_string())
    .bind(agent_id.to_string())
    .bind(&external_session_ref)
    .bind(now.to_rfc3339())
    .execute(&mut **tx)
    .await
    .map_err(storage)?;

    let context_revision_id = current_ctx_rev
        .as_deref()
        .map(|value| parse_id(value.to_owned()))
        .transpose()?;
    if let Some(ctx_id) = context_revision_id {
        sqlx::query(
            "INSERT INTO run_context_revisions(run_id,context_revision_id,pinned_at) VALUES(?,?,?)",
        )
        .bind(id.to_string())
        .bind(ctx_id.to_string())
        .bind(now.to_rfc3339())
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
    }

    append_event_tx(
        tx,
        "agent_instance",
        &agent_id.to_string(),
        "run",
        id,
        "run.started",
        json!({"task_id":task_id,"assignment_id":assignment_id,"intake_only":!enforce_intake}),
        None,
    )
    .await?;

    Ok(Run {
        id,
        task_id,
        assignment_id,
        agent_instance_id: agent_id,
        external_session_ref,
        status: "running".into(),
        stop_reason: None,
        failure_reason: None,
        checkpoint: None,
        result: None,
        started_at: now,
        ended_at: None,
        context_revision_id,
    })
}

// Reuse the exact claim checks for operator assignment and approved employee requests.
async fn claim_task_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: Id,
    agent_id: Id,
    role: &str,
    lease_seconds: i64,
    intake_required: bool,
) -> Result<Assignment, DomainError> {
    let now = Utc::now();
    if continuation::predecessor_runtime_active(&mut *tx, task_id).await? {
        return Err(DomainError::Conflict(
            "previous execution is still stopping or cleaning up".into(),
        ));
    }
    let expires = now + Duration::seconds(lease_seconds.max(30));
    let id = Uuid::new_v4();
    if role.trim().is_empty() {
        return Err(DomainError::InvalidInput("role cannot be empty".into()));
    }
    if role == "executor" {
        let blocked: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task_dependencies d JOIN tasks t ON t.id=d.depends_on_task_id WHERE d.task_id=? AND t.state!='done')").bind(task_id.to_string()).fetch_one(&mut **tx).await.map_err(storage)?;
        if blocked {
            return Err(DomainError::Conflict(
                "task has unfinished dependencies".into(),
            ));
        }
    }
    let cleanup_blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runs WHERE task_id=? AND status IN ('interrupted','cleanup_pending','cancelling'))",
        )
        .bind(task_id.to_string()).fetch_one(&mut **tx).await.map_err(storage)?;
    if cleanup_blocked {
        return Err(DomainError::Conflict(
            "Task has an interrupted or cleaning Run".into(),
        ));
    }

    let intake_required = intake_required && role == "executor";
    let initial_phase = if intake_required {
        morrows_core::INTAKE_PHASE_CONTEXT_REVIEW
    } else {
        morrows_core::INTAKE_PHASE_IMPLEMENTING
    };

    // BEGIN IMMEDIATE keeps prerequisite checks and the role claim atomic.
    // The unique active-role index remains the final ownership constraint.
    let result = sqlx::query(
            "INSERT INTO assignments(id,task_id,role,agent_instance_id,status,phase,acquired_at,expires_at,renewed_at)
             SELECT ?, id, ?, ?, 'active', ?, ?, ?, ? FROM tasks
             WHERE id=? AND state NOT IN ('done','cancelled')"
        )
            .bind(id.to_string())
            .bind(role)
            .bind(agent_id.to_string())
            .bind(initial_phase)
            .bind(now.to_rfc3339())
            .bind(expires.to_rfc3339())
            .bind(now.to_rfc3339())
            .bind(task_id.to_string())
            .execute(&mut **tx).await;

    match result {
        Ok(r) if r.rows_affected() == 0 => {
            let state: Option<String> = sqlx::query_scalar("SELECT state FROM tasks WHERE id=?")
                .bind(task_id.to_string())
                .fetch_optional(&mut **tx)
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

    if intake_required {
        intake::create_assignment_intake_tx(tx, id, task_id, agent_id).await?;
    }

    sqlx::query("UPDATE tasks SET state=CASE WHEN state='ready' THEN 'in_progress' ELSE state END, updated_at=? WHERE id=?")
            .bind(now.to_rfc3339()).bind(task_id.to_string()).execute(&mut **tx).await.map_err(storage)?;
    append_event_tx(
        tx,
        "agent_instance",
        &agent_id.to_string(),
        "task",
        task_id,
        "assignment.claimed",
        json!({"assignment_id":id,"role":role,"expires_at":expires}),
        None,
    )
    .await?;

    Ok(Assignment {
        id,
        task_id,
        role: role.to_owned(),
        agent_instance_id: agent_id,
        status: "active".into(),
        phase: initial_phase.into(),
        acquired_at: now,
        expires_at: expires,
        renewed_at: now,
    })
}

mod assignment_request;
mod intake;
