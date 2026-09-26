use super::*;
use morrows_core::{CompletionReadiness, CompletionReport, completion_criteria};
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
        completion_check_conn(&mut tx, &run, result).await
    }
}

/// Preview and completion share this validator. Completion calls it under its
/// writer transaction so changed criteria, evidence or dependencies cannot slip
/// between validation and the state transition. A preview never reserves state.
pub(super) async fn completion_check_conn(
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
    Ok(CompletionReadiness {
        run_id:run.id, task_id:task.id, context_revision_id:task.current_context_revision_id,
        criteria, completion_required:required, ready:blockers.is_empty(), blockers,
        completion_template:template,
        verification_limit:"Validates report structure, current context, task evidence references and lifecycle; does not execute tests or independently verify artifact contents or scientific claims.".into(),
    })
}
