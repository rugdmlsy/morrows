use super::*;
use morrows_core::{CreateRunMilestone, RunMilestone};
use std::collections::HashSet;

fn required(value: &str, field: &str) -> Result<(), DomainError> {
    if value.trim().is_empty() {
        return Err(DomainError::InvalidInput(format!(
            "{field} cannot be empty"
        )));
    }
    Ok(())
}

impl Store {
    /// Persist an immutable recovery milestone and atomically make it the Run's
    /// latest checkpoint. Milestones are intentionally explicit: Morrows cannot
    /// reconstruct unrecorded reasoning from provider-private context after loss.
    pub async fn create_run_milestone(
        &self,
        run_id: Id,
        actor: Id,
        input: CreateRunMilestone,
    ) -> Result<RunMilestone, DomainError> {
        required(&input.summary, "summary")?;
        required(&input.next_step, "next_step")?;
        if input.next_plan.is_empty() {
            return Err(DomainError::InvalidInput(
                "next_plan must contain at least one ordered recovery step".into(),
            ));
        }
        if !matches!(
            input.kind.as_str(),
            "milestone" | "budget_pressure" | "handoff_preparation" | "manual"
        ) {
            return Err(DomainError::InvalidInput(
                "milestone kind must be milestone, budget_pressure, handoff_preparation, or manual"
                    .into(),
            ));
        }
        for item in input
            .completed
            .iter()
            .chain(&input.verified)
            .chain(&input.remaining)
            .chain(&input.blockers)
            .chain(&input.next_plan)
            .chain(&input.execution_locations)
        {
            required(item, "milestone item")?;
        }

        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(run_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("run {run_id}")))?,
        )?;
        if run.agent_instance_id != actor || !matches!(run.status.as_str(), "running" | "paused") {
            return Err(DomainError::Conflict(
                "milestone requires a live run owned by this agent".into(),
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
        {
            return Err(DomainError::Conflict("assignment is not active".into()));
        }

        let context_revision_id: Option<Id> = sqlx::query_scalar::<_, Option<String>>(
            "SELECT current_context_revision_id FROM tasks WHERE id=?",
        )
        .bind(run.task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?
        .map(parse_id)
        .transpose()?;

        for (table, ids) in [
            ("artifacts", &input.artifact_ids),
            ("decisions", &input.decision_ids),
        ] {
            let mut seen = HashSet::new();
            for raw in ids {
                let id = Id::parse_str(raw)
                    .map_err(|_| DomainError::InvalidInput("invalid reference UUID".into()))?;
                if !seen.insert(id) {
                    return Err(DomainError::InvalidInput(
                        "duplicate milestone reference".into(),
                    ));
                }
                let found: bool = sqlx::query_scalar(&format!(
                    "SELECT EXISTS(SELECT 1 FROM {table} WHERE id=? AND task_id=?)"
                ))
                .bind(id.to_string())
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

        let sequence: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(sequence),0)+1 FROM run_milestones WHERE run_id=?",
        )
        .bind(run_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        let id = Id::new_v4();
        let now = Utc::now();
        sqlx::query(
            "INSERT INTO run_milestones(id,run_id,task_id,created_by,sequence,context_revision_id,content_json,created_at)
             VALUES(?,?,?,?,?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(run_id.to_string())
        .bind(run.task_id.to_string())
        .bind(actor.to_string())
        .bind(sequence)
        .bind(context_revision_id.map(|id| id.to_string()))
        .bind(serde_json::to_string(&input).map_err(storage)?)
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;

        for (table, column, ids) in [
            (
                "run_milestone_artifacts",
                "artifact_id",
                &input.artifact_ids,
            ),
            (
                "run_milestone_decisions",
                "decision_id",
                &input.decision_ids,
            ),
        ] {
            for raw in ids {
                sqlx::query(&format!(
                    "INSERT INTO {table}(milestone_id,{column}) VALUES(?,?)"
                ))
                .bind(id.to_string())
                .bind(raw)
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
            }
        }

        let checkpoint = json!({
            "milestone_id": id,
            "sequence": sequence,
            "context_revision_id": context_revision_id,
            "kind": input.kind,
            "summary": input.summary,
            "completed": input.completed,
            "verified": input.verified,
            "remaining": input.remaining,
            "blockers": input.blockers,
            "next_step": input.next_step,
            "next_plan": input.next_plan,
            "execution_locations": input.execution_locations,
            "artifact_ids": input.artifact_ids,
            "decision_ids": input.decision_ids,
        });
        sqlx::query("UPDATE runs SET checkpoint_json=? WHERE id=?")
            .bind(checkpoint.to_string())
            .bind(run_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor.to_string(),
            "run",
            run_id,
            "run.milestone_created",
            json!({"task_id":run.task_id,"milestone_id":id,"sequence":sequence,"context_revision_id":context_revision_id}),
            None,
        )
        .await?;
        // Preserve the existing checkpoint timestamp/recovery contract.
        append_event_tx(
            &mut tx,
            "agent_instance",
            &actor.to_string(),
            "run",
            run_id,
            "run.checkpointed",
            json!({"task_id":run.task_id,"milestone_id":id,"sequence":sequence}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;

        Ok(RunMilestone {
            id,
            run_id,
            task_id: run.task_id,
            created_by: actor,
            sequence,
            context_revision_id,
            content: input,
            created_at: now,
        })
    }

    pub async fn get_run_milestone(&self, id: Id) -> Result<RunMilestone, DomainError> {
        sqlx::query("SELECT * FROM run_milestones WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("milestone {id}")))
            .and_then(row_to_run_milestone)
    }

    pub async fn run_milestones(&self, run_id: Id) -> Result<Vec<RunMilestone>, DomainError> {
        sqlx::query("SELECT * FROM run_milestones WHERE run_id=? ORDER BY sequence")
            .bind(run_id.to_string())
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
            .into_iter()
            .map(row_to_run_milestone)
            .collect()
    }
}

pub(super) fn row_to_run_milestone(
    row: sqlx::sqlite::SqliteRow,
) -> Result<RunMilestone, DomainError> {
    Ok(RunMilestone {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        run_id: parse_id(row.try_get("run_id").map_err(storage)?)?,
        task_id: parse_id(row.try_get("task_id").map_err(storage)?)?,
        created_by: parse_id(row.try_get("created_by").map_err(storage)?)?,
        sequence: row.try_get("sequence").map_err(storage)?,
        context_revision_id: parse_opt_id(row.try_get("context_revision_id").map_err(storage)?)?,
        content: serde_json::from_str(&row.try_get::<String, _>("content_json").map_err(storage)?)
            .map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}
