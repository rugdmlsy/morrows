use super::*;
use morrows_core::{AcceptanceCriterion, CompletionReadiness, CriterionStatus, VerificationMode};

impl Store {
    /// Convert retained context contracts once at startup. Subsequent writes use
    /// the same importer; native contracts never take definitions from context.
    pub async fn migrate_legacy_acceptance(&self) -> Result<(), DomainError> {
        let rows = sqlx::query("SELECT t.id,c.constraints_json FROM tasks t JOIN context_revisions c ON c.id=t.current_context_revision_id WHERE t.acceptance_criteria_json='[]'")
            .fetch_all(&self.pool).await.map_err(storage)?;
        for row in rows {
            let id = parse_id(row.try_get("id").map_err(storage)?)?;
            let constraints: Value = serde_json::from_str(
                &row.try_get::<String, _>("constraints_json")
                    .map_err(storage)?,
            )
            .map_err(storage)?;
            let mut tx = self
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(storage)?;
            reconcile_legacy_conn(&mut tx, id, &constraints).await?;
            tx.commit().await.map_err(storage)?;
        }
        Ok(())
    }

    pub async fn completion_records(&self, task_id: Id) -> Result<Vec<Value>, DomainError> {
        let rows: Vec<String> = sqlx::query_scalar(
            "SELECT record_json FROM completion_records WHERE task_id=? ORDER BY created_at DESC",
        )
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter()
            .map(|s| serde_json::from_str(&s).map_err(storage))
            .collect()
    }

    pub async fn verification_receipts(&self, task_id: Id) -> Result<Vec<Value>, DomainError> {
        let rows = sqlx::query(
            "SELECT * FROM verification_receipts WHERE task_id=? ORDER BY created_at,id",
        )
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(receipt_value).collect()
    }

    /// Reserve an attempt before contacting runtime. A lost response leaves a
    /// visible BLOCKED attempt, and cannot silently cause a duplicate effect.
    pub async fn begin_verification(
        &self,
        run_id: Id,
        agent: Id,
        criterion_id: &str,
        retry_reason: Option<&str>,
    ) -> Result<(String, AcceptanceCriterion), DomainError> {
        self.authorize_run_runtime_access(run_id, agent).await?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let run = live_run_conn(&mut tx, run_id, agent, "executor").await?;
        let task = task_conn(&mut tx, run.task_id).await?;
        let c = task
            .acceptance_criteria
            .iter()
            .find(|c| c.id == criterion_id)
            .cloned()
            .ok_or_else(|| DomainError::InvalidInput("unknown criterion".into()))?;
        if !matches!(
            c.verification,
            VerificationMode::DeterministicCheck | VerificationMode::ArtifactCheck
        ) {
            return Err(DomainError::InvalidInput(
                "criterion does not use a runtime check".into(),
            ));
        }
        let previous: Option<String> = sqlx::query_scalar("SELECT verdict FROM verification_receipts WHERE executor_run_id=? AND criterion_id=? AND kind=? ORDER BY created_at DESC,id DESC LIMIT 1")
            .bind(run_id.to_string()).bind(criterion_id).bind(mode_name(&c.verification)).fetch_optional(&mut *tx).await.map_err(storage)?;
        if let Some(verdict) = previous {
            if verdict == "BLOCKED" {
                return Err(DomainError::Conflict("previous attempt has no reconciled terminal outcome; inspect receipt before retry".into()));
            }
            if retry_reason.is_none_or(|s| s.trim().is_empty()) {
                return Err(DomainError::InvalidInput(
                    "retry requires a reason describing a repair or environment change".into(),
                ));
            }
        }
        let id = Uuid::new_v4().to_string();
        let detail = json!({"retry_reason":retry_reason,"check":c.check,"actor":agent,"started_at":Utc::now()});
        insert_receipt_conn(
            &mut tx,
            &id,
            &task,
            &run,
            None,
            &c,
            mode_name(&c.verification),
            "BLOCKED",
            detail,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok((id, c))
    }

    pub async fn finish_verification(
        &self,
        id: &str,
        verdict: &str,
        detail: Value,
    ) -> Result<Value, DomainError> {
        if !matches!(verdict, "PASS" | "FAIL" | "BLOCKED") {
            return Err(DomainError::InvalidInput("invalid check verdict".into()));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row = sqlx::query("SELECT * FROM verification_receipts WHERE id=?")
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .map_err(storage)?;
        let mut original: Value =
            serde_json::from_str(&row.try_get::<String, _>("detail_json").map_err(storage)?)
                .map_err(storage)?;
        original["outcome"] = detail;
        original["finished_at"] = json!(Utc::now());
        sqlx::query("UPDATE verification_receipts SET verdict=?,detail_json=? WHERE id=?")
            .bind(verdict)
            .bind(original.to_string())
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        let task_id = parse_id(row.try_get("task_id").map_err(storage)?)?;
        Ok(self
            .verification_receipts(task_id)
            .await?
            .into_iter()
            .find(|v| v["id"] == id)
            .expect("receipt was persisted"))
    }

    /// Reviewers receive only shared work state. Provider conversation handles
    /// and private executor checkpoints are deliberately absent from this view.
    pub async fn review_context(
        &self,
        reviewer_run_id: Id,
        agent: Id,
    ) -> Result<Value, DomainError> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let run = live_run_conn(&mut tx, reviewer_run_id, agent, "reviewer").await?;
        let task = task_conn(&mut tx, run.task_id).await?;
        drop(tx);
        let context = if let Some(id) = task.current_context_revision_id {
            Some(self.get_context_revision(id).await?)
        } else {
            None
        };
        Ok(
            json!({"task":task,"context":context,"artifacts":self.task_artifacts(run.task_id).await?,
            "decisions":self.task_decisions(run.task_id).await?,"verification_receipts":self.verification_receipts(run.task_id).await?}),
        )
    }

    pub async fn submit_review(
        &self,
        reviewer_run_id: Id,
        agent: Id,
        executor_run_id: Id,
        criterion_id: &str,
        verdict: &str,
        rationale: &str,
        artifact_ids: &[String],
        expected_context_revision_id: Option<Id>,
        expected_acceptance_version: i64,
        operator_actor: Option<Value>,
    ) -> Result<Value, DomainError> {
        if !matches!(verdict, "PASS" | "FAIL" | "BLOCKED")
            || rationale.trim().is_empty()
            || artifact_ids.is_empty()
        {
            return Err(DomainError::InvalidInput(
                "review needs PASS/FAIL/BLOCKED, findings/rationale and artifacts".into(),
            ));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let reviewer = live_run_conn(&mut tx, reviewer_run_id, agent, "reviewer").await?;
        let executor = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(executor_run_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?,
        )?;
        if reviewer.task_id != executor.task_id
            || reviewer.agent_instance_id == executor.agent_instance_id
            || !matches!(executor.status.as_str(), "running" | "paused")
        {
            return Err(DomainError::Conflict(
                "review requires a live same-task executor owned by another Agent".into(),
            ));
        }
        let role: String = sqlx::query_scalar("SELECT role FROM assignments WHERE id=?")
            .bind(executor.assignment_id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage)?;
        if role != "executor" {
            return Err(DomainError::InvalidInput(
                "target Run is not an executor".into(),
            ));
        }
        let task = task_conn(&mut tx, reviewer.task_id).await?;
        if task.current_context_revision_id != expected_context_revision_id
            || task.acceptance_version != expected_acceptance_version
        {
            return Err(DomainError::Conflict(
                "review snapshot is stale; read review_context and review again".into(),
            ));
        }
        let c = task
            .acceptance_criteria
            .iter()
            .find(|c| c.id == criterion_id)
            .ok_or_else(|| DomainError::InvalidInput("unknown criterion".into()))?;
        if c.verification != VerificationMode::IndependentReview && !c.requires_independent_review {
            return Err(DomainError::InvalidInput(
                "criterion does not require independent review".into(),
            ));
        }
        if verdict == "PASS" {
            for check in &task.acceptance_criteria {
                if matches!(
                    check.verification,
                    VerificationMode::DeterministicCheck | VerificationMode::ArtifactCheck
                ) {
                    let latest:Option<String>=sqlx::query_scalar("SELECT verdict FROM verification_receipts WHERE executor_run_id=? AND criterion_id=? AND context_revision_id IS ? AND acceptance_version=? AND kind=? ORDER BY created_at DESC,id DESC LIMIT 1").bind(executor.id.to_string()).bind(&check.id).bind(task.current_context_revision_id.map(|v|v.to_string())).bind(task.acceptance_version).bind(mode_name(&check.verification)).fetch_optional(&mut *tx).await.map_err(storage)?;
                    if latest.as_deref().is_some_and(|v| {
                        v != "PASS" && !(v == "FAIL" && check.allow_not_applicable)
                    }) {
                        return Err(DomainError::Conflict(
                            "PASS review cannot precede a pending or failed required runtime check"
                                .into(),
                        ));
                    }
                }
            }
        }
        validate_artifacts_conn(&mut tx, task.id, artifact_ids).await?;
        let id = Uuid::new_v4().to_string();
        let detail = json!({"rationale":rationale,"artifact_ids":artifact_ids,"reviewer_agent":agent,"operator_actor":operator_actor});
        insert_receipt_conn(
            &mut tx,
            &id,
            &task,
            &executor,
            Some(reviewer_run_id),
            c,
            "independent_review",
            verdict,
            detail,
        )
        .await?;
        sqlx::query("UPDATE tasks SET state=?,updated_at=? WHERE id=?")
            .bind(if verdict == "FAIL" {
                "in_progress"
            } else {
                "review"
            })
            .bind(Utc::now().to_rfc3339())
            .bind(task.id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(&mut tx,"agent_instance",&agent.to_string(),"task",task.id,"criterion.reviewed",json!({"receipt_id":id,"criterion_id":criterion_id,"verdict":verdict,"reviewer_run_id":reviewer_run_id}),None).await?;
        tx.commit().await.map_err(storage)?;
        Ok(json!({"id":id,"verdict":verdict,"criterion_id":criterion_id}))
    }

    /// Human verdicts have an operator-only server route, never an employee MCP
    /// route. The authenticated operator is recorded in the receipt.
    pub async fn submit_human_review(
        &self,
        task_id: Id,
        executor_run_id: Id,
        criterion_id: &str,
        verdict: &str,
        rationale: &str,
        actor: &str,
        expected_context_revision_id: Option<Id>,
        expected_acceptance_version: i64,
    ) -> Result<Value, DomainError> {
        if !matches!(verdict, "PASS" | "FAIL" | "BLOCKED") || rationale.trim().is_empty() {
            return Err(DomainError::InvalidInput(
                "human review requires verdict and rationale".into(),
            ));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let task = task_conn(&mut tx, task_id).await?;
        if task.current_context_revision_id != expected_context_revision_id
            || task.acceptance_version != expected_acceptance_version
        {
            return Err(DomainError::Conflict(
                "human review snapshot is stale; refresh Task before approval".into(),
            ));
        }
        let run = row_to_run(
            sqlx::query("SELECT * FROM runs WHERE id=?")
                .bind(executor_run_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?,
        )?;
        if run.task_id != task_id || !matches!(run.status.as_str(), "running" | "paused") {
            return Err(DomainError::Conflict(
                "human review requires a live same-task Run".into(),
            ));
        }
        let c = task
            .acceptance_criteria
            .iter()
            .find(|c| c.id == criterion_id && c.verification == VerificationMode::HumanReview)
            .ok_or_else(|| DomainError::InvalidInput("unknown human review criterion".into()))?;
        let id = Uuid::new_v4().to_string();
        insert_receipt_conn(
            &mut tx,
            &id,
            &task,
            &run,
            None,
            c,
            "human_review",
            verdict,
            json!({"actor":actor,"rationale":rationale}),
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(json!({"id":id,"verdict":verdict}))
    }
}

fn mode_name(m: &VerificationMode) -> &'static str {
    match m {
        VerificationMode::SelfAttested => "self_attested",
        VerificationMode::DeterministicCheck => "deterministic_check",
        VerificationMode::ArtifactCheck => "artifact_check",
        VerificationMode::IndependentReview => "independent_review",
        VerificationMode::HumanReview => "human_review",
    }
}
async fn task_conn(conn: &mut sqlx::SqliteConnection, id: Id) -> Result<Task, DomainError> {
    row_to_task(
        sqlx::query("SELECT * FROM tasks WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&mut *conn)
            .await
            .map_err(storage)?,
    )
}
async fn live_run_conn(
    conn: &mut sqlx::SqliteConnection,
    id: Id,
    agent: Id,
    role: &str,
) -> Result<Run, DomainError> {
    let run = row_to_run(
        sqlx::query("SELECT * FROM runs WHERE id=?")
            .bind(id.to_string())
            .fetch_one(&mut *conn)
            .await
            .map_err(storage)?,
    )?;
    let a = row_to_assignment(
        sqlx::query("SELECT * FROM assignments WHERE id=?")
            .bind(run.assignment_id.to_string())
            .fetch_one(&mut *conn)
            .await
            .map_err(storage)?,
    )?;
    if run.agent_instance_id != agent
        || a.role != role
        || a.status != "active"
        || a.expires_at <= Utc::now()
        || run.status != "running"
        || a.phase != "implementing"
    {
        return Err(DomainError::Conflict(
            "live owned Run with the required role is required".into(),
        ));
    }
    Ok(run)
}
async fn insert_receipt_conn(
    conn: &mut sqlx::SqliteConnection,
    id: &str,
    task: &Task,
    run: &Run,
    reviewer: Option<Id>,
    c: &AcceptanceCriterion,
    kind: &str,
    verdict: &str,
    detail: Value,
) -> Result<(), DomainError> {
    sqlx::query("INSERT INTO verification_receipts(id,task_id,executor_run_id,reviewer_run_id,criterion_id,acceptance_version,context_revision_id,kind,verdict,detail_json,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
        .bind(id).bind(task.id.to_string()).bind(run.id.to_string()).bind(reviewer.map(|id|id.to_string())).bind(&c.id).bind(task.acceptance_version)
        .bind(task.current_context_revision_id.map(|id|id.to_string())).bind(kind).bind(verdict).bind(detail.to_string()).bind(Utc::now().to_rfc3339())
        .execute(&mut *conn).await.map_err(storage)?;
    Ok(())
}
fn receipt_value(row: sqlx::sqlite::SqliteRow) -> Result<Value, DomainError> {
    Ok(
        json!({"id":row.try_get::<String,_>("id").map_err(storage)?,"task_id":row.try_get::<String,_>("task_id").map_err(storage)?,
        "executor_run_id":row.try_get::<String,_>("executor_run_id").map_err(storage)?,"reviewer_run_id":row.try_get::<Option<String>,_>("reviewer_run_id").map_err(storage)?,
        "criterion_id":row.try_get::<String,_>("criterion_id").map_err(storage)?,"acceptance_version":row.try_get::<i64,_>("acceptance_version").map_err(storage)?,
        "context_revision_id":row.try_get::<Option<String>,_>("context_revision_id").map_err(storage)?,"kind":row.try_get::<String,_>("kind").map_err(storage)?,
        "verdict":row.try_get::<String,_>("verdict").map_err(storage)?,"detail":serde_json::from_str::<Value>(&row.try_get::<String,_>("detail_json").map_err(storage)?).map_err(storage)?,
        "created_at":row.try_get::<String,_>("created_at").map_err(storage)?}),
    )
}
pub(super) async fn validate_artifacts_conn(
    conn: &mut sqlx::SqliteConnection,
    task: Id,
    ids: &[String],
) -> Result<(), DomainError> {
    for id in ids {
        let valid:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifacts WHERE id=? AND task_id=? AND length(trim(uri))>0)").bind(id).bind(task.to_string()).fetch_one(&mut *conn).await.map_err(storage)?;
        if !valid {
            return Err(DomainError::InvalidInput(format!(
                "artifact {id} is missing or belongs to another task"
            )));
        }
    }
    Ok(())
}
pub(super) async fn reconcile_legacy_conn(
    conn: &mut sqlx::SqliteConnection,
    id: Id,
    constraints: &Value,
) -> Result<(), DomainError> {
    let task = task_conn(conn, id).await?;
    // Preserve the old context-defined contract behavior, but expose only one
    // normalized Task model to readers and validators. Native criteria are fixed.
    if !task.acceptance_criteria.is_empty()
        && task
            .acceptance_criteria
            .iter()
            .any(|c| c.legacy_path.is_none())
    {
        if !morrows_core::completion_criteria(constraints).is_empty() {
            return Err(DomainError::Conflict(
                "native Task criteria are immutable; context cannot define a second contract"
                    .into(),
            ));
        }
        return Ok(());
    }
    let criteria = morrows_core::import_legacy_acceptance(constraints);
    if criteria != task.acceptance_criteria {
        sqlx::query("UPDATE tasks SET acceptance_criteria_json=?,acceptance_version=acceptance_version+1 WHERE id=?").bind(serde_json::to_string(&criteria).map_err(storage)?).bind(id.to_string()).execute(&mut *conn).await.map_err(storage)?;
    }
    Ok(())
}

/// Native completion uses server receipts, never executor-supplied receipt
/// content. Context/version/Run bindings make previous attempts ineligible.
pub(super) async fn criterion_statuses_conn(
    conn: &mut sqlx::SqliteConnection,
    task: &Task,
    run: &Run,
    result: &Value,
) -> Result<Vec<CriterionStatus>, DomainError> {
    let checks = result["completion"]["checks"].as_array();
    let mut statuses = Vec::new();
    for c in &task.acceptance_criteria {
        let path = c
            .legacy_path
            .clone()
            .unwrap_or_else(|| format!("/acceptance_criteria/{}", c.id));
        let check = checks.and_then(|a| a.iter().find(|v| v["criterion_path"] == path));
        let mut missing = Vec::new();
        let mut verdict = "BLOCKED".to_string();
        let mut receipts = Vec::new();
        if let Some(check) = check {
            let status = check["status"].as_str().unwrap_or("");
            if status == "NOT_APPLICABLE"
                && c.allow_not_applicable
                && check["rationale"]
                    .as_str()
                    .is_some_and(|s| !s.trim().is_empty())
            {
                statuses.push(CriterionStatus {
                    criterion_id: c.id.clone(),
                    verdict: "NOT_APPLICABLE".into(),
                    missing_evidence: vec![],
                    receipt_ids: vec![],
                });
                continue;
            }
            if matches!(status, "passed" | "PASS") {
                verdict = "PASS".into();
            } else if matches!(status, "failed" | "FAIL") {
                verdict = "FAIL".into();
            } else {
                missing.push("PASS evidence".into());
            }
            if check["rationale"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
            {
                missing.push("rationale".into());
            }
            let ids: Vec<String> = check["artifact_ids"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default();
            if ids.is_empty() {
                missing.push("same-task artifacts".into());
            }
            for kind in &c.required_artifact_kinds {
                let mut found = false;
                for id in &ids {
                    let yes:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM artifacts WHERE id=? AND task_id=? AND kind=?)").bind(id).bind(task.id.to_string()).bind(kind).fetch_one(&mut *conn).await.map_err(storage)?;
                    found |= yes;
                }
                if !found {
                    missing.push(format!("artifact kind {kind}"));
                }
            }
        } else {
            missing.push("completion evidence".into());
        }
        let mut modes = Vec::new();
        if c.verification != VerificationMode::SelfAttested {
            modes.push(mode_name(&c.verification));
        }
        if c.requires_independent_review && c.verification != VerificationMode::IndependentReview {
            modes.push("independent_review");
        }
        for mode in modes {
            let row=sqlx::query("SELECT * FROM verification_receipts WHERE executor_run_id=? AND criterion_id=? AND acceptance_version=? AND context_revision_id IS ? AND kind=? ORDER BY created_at DESC,id DESC LIMIT 1")
                .bind(run.id.to_string()).bind(&c.id).bind(task.acceptance_version).bind(task.current_context_revision_id.map(|v|v.to_string())).bind(mode).fetch_optional(&mut *conn).await.map_err(storage)?;
            if let Some(row) = row {
                let v: String = row.try_get("verdict").map_err(storage)?;
                receipts.push(row.try_get("id").map_err(storage)?);
                if mode == "independent_review" && v == "PASS" {
                    let detail: Value = serde_json::from_str(
                        &row.try_get::<String, _>("detail_json").map_err(storage)?,
                    )
                    .map_err(storage)?;
                    let mut reviewed: Vec<String> = detail["artifact_ids"]
                        .as_array()
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    let mut proposed: Vec<String> = check
                        .and_then(|v| v["artifact_ids"].as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|v| v.as_str().map(str::to_owned))
                                .collect()
                        })
                        .unwrap_or_default();
                    reviewed.sort();
                    proposed.sort();
                    if reviewed != proposed {
                        missing.push("reviewed evidence differs from completion evidence".into());
                    }
                    let newer:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM verification_receipts WHERE executor_run_id=? AND kind IN ('deterministic_check','artifact_check') AND created_at>?)").bind(run.id.to_string()).bind(row.try_get::<String,_>("created_at").map_err(storage)?).fetch_one(&mut *conn).await.map_err(storage)?;
                    let later_finish:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM verification_receipts WHERE executor_run_id=? AND kind IN ('deterministic_check','artifact_check') AND json_extract(detail_json,'$.finished_at')>?)").bind(run.id.to_string()).bind(row.try_get::<String,_>("created_at").map_err(storage)?).fetch_one(&mut *conn).await.map_err(storage)?;
                    if newer || later_finish {
                        missing.push("independent review predates a later verification attempt; review the repaired state".into());
                    }
                }
                if v != "PASS" {
                    verdict = v.clone();
                    missing.push(format!("{mode} receipt is {v}"));
                }
            } else {
                missing.push(format!("{mode} receipt"));
            }
        }
        if !missing.is_empty() && verdict == "PASS" {
            verdict = "BLOCKED".into();
        }
        statuses.push(CriterionStatus {
            criterion_id: c.id.clone(),
            verdict,
            missing_evidence: missing,
            receipt_ids: receipts,
        });
    }
    Ok(statuses)
}

pub(super) async fn save_completion_conn(
    conn: &mut sqlx::SqliteConnection,
    run: &Run,
    result: &Value,
    readiness: &CompletionReadiness,
) -> Result<(), DomainError> {
    let role: String = sqlx::query_scalar("SELECT role FROM assignments WHERE id=?")
        .bind(run.assignment_id.to_string())
        .fetch_one(&mut *conn)
        .await
        .map_err(storage)?;
    if role != "executor" {
        return Ok(());
    }
    let task = task_conn(conn, run.task_id).await?;
    let rows = sqlx::query("SELECT * FROM verification_receipts WHERE executor_run_id=?")
        .bind(run.id.to_string())
        .fetch_all(&mut *conn)
        .await
        .map_err(storage)?;
    let receipts: Vec<Value> = rows
        .into_iter()
        .map(receipt_value)
        .collect::<Result<_, _>>()?;
    let record = json!({"criteria_snapshot":task.acceptance_criteria,"acceptance_version":task.acceptance_version,"context_revision_id":task.current_context_revision_id,
        "criterion_verdicts":readiness.criterion_statuses,"evidence":result.get("completion"),"verification_receipts":receipts,
        "unresolved_items":[],"waived_items":readiness.criterion_statuses.iter().filter(|s|s.verdict=="NOT_APPLICABLE").collect::<Vec<_>>(),
        "completed_at":Utc::now(),"actor":run.agent_instance_id,"run_id":run.id,"task_id":task.id,"memory_disposition":result.get("memory_disposition")});
    sqlx::query("INSERT INTO completion_records(id,task_id,run_id,record_json,created_at) VALUES(?,?,?,?,?)").bind(Uuid::new_v4().to_string()).bind(task.id.to_string()).bind(run.id.to_string()).bind(record.to_string()).bind(Utc::now().to_rfc3339()).execute(&mut *conn).await.map_err(storage)?;
    Ok(())
}

impl Store {
    /// An operator may reconcile a lost check response only as FAIL, enabling a
    /// new actual verification. Reconciliation cannot fabricate a passing check.
    pub async fn reconcile_verification(
        &self,
        task_id: Id,
        id: &str,
        reason: &str,
        actor: Value,
    ) -> Result<Value, DomainError> {
        if reason.trim().is_empty() {
            return Err(DomainError::InvalidInput(
                "reconciliation requires observed outcome and retry rationale".into(),
            ));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row = sqlx::query("SELECT * FROM verification_receipts WHERE id=? AND task_id=?")
            .bind(id)
            .bind(task_id.to_string())
            .fetch_one(&mut *tx)
            .await
            .map_err(storage)?;
        let verdict: String = row.try_get("verdict").map_err(storage)?;
        if verdict != "BLOCKED" {
            return Err(DomainError::Conflict(
                "only unknown BLOCKED attempts can be reconciled".into(),
            ));
        }
        let mut detail: Value =
            serde_json::from_str(&row.try_get::<String, _>("detail_json").map_err(storage)?)
                .map_err(storage)?;
        if detail.get("finished_at").is_none() {
            return Err(DomainError::Conflict(
                "attempt is still running; wait for a terminal or lost-response outcome".into(),
            ));
        }
        detail["reconciliation"] = json!({"reason":reason,"actor":actor,"at":Utc::now(),"verdict":"FAIL","original_outcome_preserved":true});
        sqlx::query("UPDATE verification_receipts SET verdict='FAIL',detail_json=? WHERE id=?")
            .bind(detail.to_string())
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "human",
            &actor.to_string(),
            "task",
            task_id,
            "verification.reconciled",
            json!({"receipt_id":id,"reason":reason}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(json!({"id":id,"verdict":"FAIL","reconciliation":detail["reconciliation"]}))
    }
}
