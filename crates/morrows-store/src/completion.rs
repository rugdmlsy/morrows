use super::*;
use morrows_core::{
    CompletionReadiness, CompletionReport, MemoryDisposition, PublishProjectMemory,
    completion_criteria,
};
use std::collections::HashSet;

impl Store {
    pub async fn run_completion_check(
        &self,
        id: Id,
        agent_id: Id,
        result: &Value,
    ) -> Result<CompletionReadiness, DomainError> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("run {id}")))?,
        )?;
        if run.agent_instance_id != agent_id {
            return Err(DomainError::Conflict(
                "run belongs to another agent instance".into(),
            ));
        }
        completion_check_conn(self, &mut tx, &run, result).await
    }
}

/// Preview and completion share this validator. Completion calls it under its
/// writer transaction so changed criteria, evidence or dependencies cannot slip
/// between validation and the state transition. A preview never reserves state.
pub(super) async fn completion_check_conn(
    store: &Store,
    conn: &mut sqlx::SqliteConnection,
    run: &Run,
    result: &Value,
) -> Result<CompletionReadiness, DomainError> {
    let task = row_to_task(
        sqlx::query("SELECT * FROM tasks WHERE id=?")
            .bind(run.task_id.to_string())
            .fetch_one(&mut *conn)
            .await
            .map_err(storage)?,
    )?;
    let assignment = row_to_assignment(
        sqlx::query("SELECT * FROM assignments WHERE id=?")
            .bind(run.assignment_id.to_string())
            .fetch_one(&mut *conn)
            .await
            .map_err(storage)?,
    )?;
    let context = match task.current_context_revision_id {
        Some(id) => Some(row_to_context_revision(
            sqlx::query("SELECT * FROM context_revisions WHERE id=?")
                .bind(id.to_string())
                .fetch_one(&mut *conn)
                .await
                .map_err(storage)?,
        )?),
        None => None,
    };
    let criteria = context
        .as_ref()
        .map(|c| completion_criteria(&c.constraints))
        .unwrap_or_default();
    let required = assignment.role == "executor" && !criteria.is_empty();
    let memory_disposition_required = assignment.role == "executor" && task.project_id.is_some();
    let mut blockers = Vec::new();
    if !matches!(run.status.as_str(), "running" | "paused") {
        blockers.push(format!("run is {}", run.status));
    }
    if assignment.status != "active" || assignment.expires_at <= Utc::now() {
        blockers.push(
            "assignment is not active; renew a live lease or use the recovery workflow".into(),
        );
    }
    if matches!(task.state, TaskState::Done | TaskState::Cancelled) {
        blockers.push(format!("task is {}", task.state));
    }
    if assignment.role == "executor" {
        let dependencies: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_dependencies d JOIN tasks t ON t.id=d.depends_on_task_id WHERE d.task_id=? AND t.state!='done'")
            .bind(task.id.to_string()).fetch_one(&mut *conn).await.map_err(storage)?;
        if dependencies > 0 {
            blockers.push(format!("task has {dependencies} unfinished dependencies"));
        }
    }
    if result.get("all_acceptance_criteria_met") == Some(&Value::Bool(false))
        || result.get("ok") == Some(&Value::Bool(false))
    {
        blockers.push(
            "result explicitly reports failure; checkpoint or hand off incomplete work".into(),
        );
    }

    if memory_disposition_required {
        match result.get("memory_disposition") {
            None => blockers.push(
                "result.memory_disposition is required for executor tasks with a project; publish/update reusable project knowledge or use not_applicable with a rationale".into(),
            ),
            Some(value) => match serde_json::from_value::<MemoryDisposition>(value.clone()) {
                Err(error) => blockers.push(format!(
                    "invalid result.memory_disposition: {error}"
                )),
                Ok(disposition) => {
                    let rationale = disposition.rationale.trim();
                    match disposition.status.as_str() {
                        "not_applicable" => {
                            if rationale.is_empty() {
                                blockers.push(
                                    "memory_disposition not_applicable requires a rationale".into(),
                                );
                            }
                            if !disposition.memory_entry_ids.is_empty() {
                                blockers.push(
                                    "memory_disposition not_applicable must not include memory_entry_ids".into(),
                                );
                            }
                        }
                        "published" | "updated" => {
                            if rationale.is_empty() {
                                blockers.push(format!(
                                    "memory_disposition {} requires a rationale",
                                    disposition.status
                                ));
                            }
                            if disposition.memory_entry_ids.is_empty() {
                                blockers.push(format!(
                                    "memory_disposition {} requires memory_entry_ids",
                                    disposition.status
                                ));
                            }
                            let project_id = task.project_id.expect(
                                "memory_disposition_required implies a project-backed task",
                            );
                            let git_backend =
                                Store::git_backend(conn, &project_id.to_string()).await?;
                            if git_backend {
                                // Also proves the SQL projection still exactly matches the
                                // authoritative Git ref before accepting cited publications.
                                store.project_memory_head_conn(conn, project_id).await?;
                            }
                            let mut seen_memory = HashSet::new();
                            for memory_id in &disposition.memory_entry_ids {
                                if !seen_memory.insert(memory_id.as_str()) {
                                    blockers.push(format!(
                                        "duplicate memory_entry_id {memory_id}"
                                    ));
                                    continue;
                                }
                                let id = match Id::parse_str(memory_id) {
                                    Ok(id) => id,
                                    Err(_) => {
                                        blockers.push(format!(
                                            "memory_entry_id {memory_id} is not a UUID"
                                        ));
                                        continue;
                                    }
                                };
                                let row = sqlx::query(
                                    "SELECT m.project_id,m.visibility,m.supersedes_memory_id,                                     p.agent_instance_id,p.idempotency_key,p.input_json                                      FROM memory_entries m                                      JOIN memory_publications p ON p.memory_id=m.id                                      WHERE m.id=?",
                                )
                                .bind(id.to_string())
                                .fetch_optional(&mut *conn)
                                .await
                                .map_err(storage)?;
                                let Some(row) = row else {
                                    blockers.push(format!(
                                        "memory_entry_id {memory_id} is not a project_memory_publish receipt"
                                    ));
                                    continue;
                                };
                                let row_project: Option<String> =
                                    row.try_get("project_id").map_err(storage)?;
                                let visibility: String =
                                    row.try_get("visibility").map_err(storage)?;
                                let supersedes: Option<String> =
                                    row.try_get("supersedes_memory_id").map_err(storage)?;
                                let actor: String =
                                    row.try_get("agent_instance_id").map_err(storage)?;
                                let retry_key: String =
                                    row.try_get("idempotency_key").map_err(storage)?;
                                let input_json: String =
                                    row.try_get("input_json").map_err(storage)?;
                                let input: PublishProjectMemory =
                                    serde_json::from_str(&input_json).map_err(storage)?;
                                if row_project.as_deref() != Some(&project_id.to_string())
                                    || visibility != "shared"
                                    || input.task_id != task.id
                                {
                                    blockers.push(format!(
                                        "memory_entry_id {memory_id} was not published by this task into its current project"
                                    ));
                                    continue;
                                }
                                if disposition.status == "updated" && supersedes.is_none() {
                                    blockers.push(format!(
                                        "memory_entry_id {memory_id} does not supersede an existing project memory"
                                    ));
                                }
                                if git_backend {
                                    let operation_state: Option<String> = sqlx::query_scalar(
                                        "SELECT state FROM memory_git_operations                                          WHERE domain=? AND kind='publication' AND actor_id=?                                          AND retry_key=? ORDER BY created_at DESC LIMIT 1",
                                    )
                                    .bind(project_id.to_string())
                                    .bind(actor)
                                    .bind(retry_key)
                                    .fetch_optional(&mut *conn)
                                    .await
                                    .map_err(storage)?;
                                    // Publications made before Git cutover legitimately have no
                                    // Git operation row; project_memory_head_conn above proves
                                    // that the mirrored entry is present in the authoritative
                                    // Git snapshot. Once an operation row exists, only indexed
                                    // is an acceptable terminal state.
                                    if operation_state
                                        .as_deref()
                                        .is_some_and(|state| state != "indexed")
                                    {
                                        blockers.push(format!(
                                            "memory_entry_id {memory_id} has a non-indexed Git publication operation"
                                        ));
                                    }
                                }
                            }
                        }
                        other => blockers.push(format!(
                            "memory_disposition.status must be published, updated, or not_applicable; got {other}"
                        )),
                    }
                }
            },
        }
    }
    match result.get("completion") {
        None if required => blockers.push(
            "result.completion is required; fill completion_template from run_completion_check"
                .into(),
        ),
        Some(value) => match serde_json::from_value::<CompletionReport>(value.clone()) {
            Err(error) => blockers.push(format!("invalid result.completion: {error}")),
            Ok(report) => {
                if Some(report.context_revision_id.as_str())
                    != task
                        .current_context_revision_id
                        .as_ref()
                        .map(|id| id.to_string())
                        .as_deref()
                {
                    blockers.push(
                        "completion context is stale; read task_context and reconcile evidence"
                            .into(),
                    );
                }
                let mut seen = HashSet::new();
                for check in &report.checks {
                    if !seen.insert(check.criterion_path.as_str()) {
                        blockers.push(format!("duplicate criterion {}", check.criterion_path));
                    }
                    if !criteria.iter().any(|c| c.path == check.criterion_path) {
                        blockers.push(format!("unknown criterion {}", check.criterion_path));
                    }
                    if check.status != "passed"
                        || check.rationale.trim().is_empty()
                        || check.artifact_ids.is_empty()
                    {
                        blockers.push(format!(
                            "{} requires passed, rationale and artifact_ids",
                            check.criterion_path
                        ));
                    }
                    for artifact_id in &check.artifact_ids {
                        let valid: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifacts WHERE id=? AND task_id=? AND length(trim(uri))>0)")
                            .bind(artifact_id).bind(task.id.to_string()).fetch_one(&mut *conn).await.map_err(storage)?;
                        if !valid {
                            blockers.push(format!("artifact {artifact_id} is missing, has no URI, or belongs to another task"));
                        }
                    }
                }
                for criterion in &criteria {
                    if !seen.contains(criterion.path.as_str()) {
                        blockers.push(format!("missing criterion {}", criterion.path));
                    }
                }
            }
        },
        _ => {}
    }
    if required
        && context
            .as_ref()
            .is_none_or(|c| c.current_summary.trim().is_empty())
    {
        blockers
            .push("persist a nonempty current_summary with memory_revise before completion".into());
    }
    let template = json!({"context_revision_id":task.current_context_revision_id,
        "checks":criteria.iter().map(|c| json!({"criterion_path":c.path,"status":"pending","rationale":"","artifact_ids":[]})).collect::<Vec<_>>()});
    let memory_disposition_template = if memory_disposition_required {
        json!({"status":"pending","rationale":"","memory_entry_ids":[]})
    } else {
        Value::Null
    };
    Ok(CompletionReadiness {
        run_id:run.id, task_id:task.id, context_revision_id:task.current_context_revision_id,
        criteria, completion_required:required, memory_disposition_required,
        ready:blockers.is_empty(), blockers,
        completion_template:template, memory_disposition_template,
        verification_limit:"Validates report structure, current context, task evidence references, explicit project-memory disposition and lifecycle; cited project memories must be source-task/project publications; Git-backed projects must have a consistent authoritative projection and any matching Git publication operation must be indexed. It does not execute tests or independently verify artifact or memory contents.".into(),
    })
}
