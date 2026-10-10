use morrows_core::*;
use morrows_store::Store;
use serde_json::{Value, json};

fn criterion(id: &str, mode: &str) -> AcceptanceCriterion {
    serde_json::from_value(json!({"id":id,"requirement":format!("Verify {id}"),"verification":mode,
        "check":if mode=="deterministic_check" {json!({"command":"test -f passed","cwd":"/tmp","machine":"morrow-macbook","read_only":true,"timeout_s":10})} else {Value::Null},
        "artifact_check":if mode=="artifact_check" {json!({"path":"/tmp/report","machine":"morrow-macbook"})} else {Value::Null}})).unwrap()
}
async fn task(store: &Store, criteria: Vec<AcceptanceCriterion>) -> Task {
    store
        .create_task(CreateTask {
            acceptance_criteria: criteria,
            project_id: None,
            title: "Acceptance fixture".into(),
            description: "spec".into(),
            owner_actor_id: "human:test".into(),
            state: TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap()
}
async fn run(store: &Store, task: &Task, agent: Id, role: &str) -> Run {
    let a = store.claim_task(task.id, agent, role, 300).await.unwrap();
    store.start_run(a.id, agent, None).await.unwrap()
}
async fn report(store: &Store, task: &Task, run: &Run, agent: Id) -> (Value, String) {
    let artifact = store
        .create_artifact(
            task.id,
            agent,
            CreateArtifact {
                title: "Evidence".into(),
                uri: "/tmp/report".into(),
                kind: "report".into(),
                description: "test output".into(),
            },
        )
        .await
        .unwrap();
    let mut template = store
        .run_completion_check(run.id, agent, &json!({}))
        .await
        .unwrap()
        .completion_template;
    for c in template["checks"].as_array_mut().unwrap() {
        c["status"] = json!("PASS");
        c["rationale"] = json!("Verified evidence");
        c["artifact_ids"] = json!([artifact.id]);
    }
    (json!({"completion":template}), artifact.id.to_string())
}

#[tokio::test]
async fn fail_fix_review_done_is_atomic_and_unlocks_successor() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    let executor = s.register_agent("executor", &[]).await.unwrap();
    let reviewer = s.register_agent("reviewer", &[]).await.unwrap();
    let t = task(
        &s,
        vec![
            criterion("test", "deterministic_check"),
            criterion("review", "independent_review"),
            criterion("report", "self_attested"),
        ],
    )
    .await;
    let next = task(&s, vec![]).await;
    s.add_dependency(next.id, t.id, executor.id).await.unwrap();
    let r = run(&s, &t, executor.id, "executor").await;
    assert!(
        s.claim_task(t.id, executor.id, "reviewer", 300)
            .await
            .is_err()
    );
    let rr = run(&s, &t, reviewer.id, "reviewer").await;
    let view = s.review_context(rr.id, reviewer.id).await.unwrap();
    assert!(view.get("provider_conversation_ref").is_none());
    assert!(view.get("checkpoint").is_none());
    let (result, artifact) = report(&s, &t, &r, executor.id).await;
    assert!(
        !s.run_completion_check(r.id, executor.id, &result)
            .await
            .unwrap()
            .ready
    );
    assert!(
        s.complete_run(r.id, executor.id, result.clone())
            .await
            .is_err()
    );
    assert_ne!(
        s.task_gate(next.id).await.unwrap().state,
        GateState::Eligible
    );
    let (id, _) = s
        .begin_verification(r.id, executor.id, "test", None)
        .await
        .unwrap();
    assert!(
        s.submit_review(
            rr.id,
            reviewer.id,
            r.id,
            "review",
            "PASS",
            "premature review",
            &[artifact.clone()],
            t.current_context_revision_id,
            t.acceptance_version,
            None
        )
        .await
        .is_err()
    );
    assert!(
        s.reconcile_verification(
            t.id,
            &id,
            "premature reconciliation",
            json!({"label":"operator"})
        )
        .await
        .is_err()
    );
    assert!(
        s.begin_verification(r.id, executor.id, "test", Some("duplicate"))
            .await
            .is_err()
    );
    s.finish_verification(&id, "FAIL", json!({"exit_status":1}))
        .await
        .unwrap();
    assert!(
        s.begin_verification(r.id, executor.id, "test", None)
            .await
            .is_err()
    );
    s.submit_review(
        rr.id,
        reviewer.id,
        r.id,
        "review",
        "FAIL",
        "Missing expected content",
        &[artifact.clone()],
        t.current_context_revision_id,
        t.acceptance_version,
        None,
    )
    .await
    .unwrap();
    assert_eq!(s.get_task(t.id).await.unwrap().state, TaskState::InProgress);
    assert!(
        s.complete_run(r.id, executor.id, result.clone())
            .await
            .is_err()
    );
    let (id, _) = s
        .begin_verification(r.id, executor.id, "test", Some("Repaired fixture"))
        .await
        .unwrap();
    s.finish_verification(&id, "PASS", json!({"exit_status":0}))
        .await
        .unwrap();
    assert!(
        s.complete_run(r.id, executor.id, result.clone())
            .await
            .is_err()
    );
    s.submit_review(
        rr.id,
        reviewer.id,
        r.id,
        "review",
        "PASS",
        "Corrected content verified",
        &[artifact],
        t.current_context_revision_id,
        t.acceptance_version,
        None,
    )
    .await
    .unwrap();
    assert!(
        s.run_completion_check(r.id, executor.id, &result)
            .await
            .unwrap()
            .ready
    );
    s.complete_run(r.id, executor.id, result).await.unwrap();
    assert_eq!(s.get_task(t.id).await.unwrap().state, TaskState::Done);
    assert_eq!(
        s.task_gate(next.id).await.unwrap().state,
        GateState::Eligible
    );
    let records = s.completion_records(t.id).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(
        records[0]["criterion_verdicts"].as_array().unwrap().len(),
        3
    );
    assert_eq!(
        records[0]["verification_receipts"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
}

#[tokio::test]
async fn receipts_and_reports_are_bound_to_current_context_and_same_task() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    let a = s.register_agent("executor", &[]).await.unwrap();
    let b = s.register_agent("reviewer", &[]).await.unwrap();
    let t = task(&s, vec![criterion("test", "deterministic_check")]).await;
    let r = run(&s, &t, a.id, "executor").await;
    let (result, _) = report(&s, &t, &r, a.id).await;
    let (id, _) = s
        .begin_verification(r.id, a.id, "test", None)
        .await
        .unwrap();
    s.finish_verification(&id, "PASS", json!({})).await.unwrap();
    assert!(
        s.run_completion_check(r.id, a.id, &result)
            .await
            .unwrap()
            .ready
    );
    let context = CreateContextRevision {
        goal: "goal".into(),
        background: "background".into(),
        constraints: json!({}),
        current_summary: "changed implementation".into(),
        created_by_actor_id: "agent:test".into(),
    };
    s.create_context_revision(t.id, context.clone())
        .await
        .unwrap();
    assert!(
        !s.run_completion_check(r.id, a.id, &result)
            .await
            .unwrap()
            .ready
    );
    let (fresh, _) = report(&s, &t, &r, a.id).await;
    assert!(
        !s.run_completion_check(r.id, a.id, &fresh)
            .await
            .unwrap()
            .ready
    );
    let mut weaken = context;
    weaken.constraints = json!({"acceptance_criteria":[],"freeze_requires":["different"]});
    assert!(s.create_context_revision(t.id, weaken).await.is_err());
    let foreign = task(&s, vec![]).await;
    let fr = run(&s, &foreign, b.id, "executor").await;
    let (_, artifact) = report(&s, &foreign, &fr, b.id).await;
    let mut wrong = fresh;
    wrong["completion"]["checks"][0]["artifact_ids"] = json!([artifact]);
    assert!(
        !s.run_completion_check(r.id, a.id, &wrong)
            .await
            .unwrap()
            .ready
    );
}

#[tokio::test]
async fn not_applicable_needs_explicit_permission_and_rationale() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    let a = s.register_agent("executor", &[]).await.unwrap();
    let mut allowed = criterion("optional", "self_attested");
    allowed.allow_not_applicable = true;
    let t = task(&s, vec![allowed, criterion("required", "self_attested")]).await;
    let r = run(&s, &t, a.id, "executor").await;
    let (mut result, _) = report(&s, &t, &r, a.id).await;
    result["completion"]["checks"][0]["status"] = json!("NOT_APPLICABLE");
    result["completion"]["checks"][0]["artifact_ids"] = json!([]);
    assert!(
        s.run_completion_check(r.id, a.id, &result)
            .await
            .unwrap()
            .ready
    );
    result["completion"]["checks"][0]["rationale"] = json!("");
    assert!(
        !s.run_completion_check(r.id, a.id, &result)
            .await
            .unwrap()
            .ready
    );
    result["completion"]["checks"][0]["rationale"] = json!("not used");
    result["completion"]["checks"][1]["status"] = json!("NOT_APPLICABLE");
    assert!(
        !s.run_completion_check(r.id, a.id, &result)
            .await
            .unwrap()
            .ready
    );
}

#[tokio::test]
async fn human_and_artifact_modes_cannot_be_self_attested() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    let a = s.register_agent("executor", &[]).await.unwrap();
    let t = task(
        &s,
        vec![
            criterion("file", "artifact_check"),
            criterion("human", "human_review"),
        ],
    )
    .await;
    let r = run(&s, &t, a.id, "executor").await;
    let (result, _) = report(&s, &t, &r, a.id).await;
    assert!(s.complete_run(r.id, a.id, result.clone()).await.is_err());
    let (id, _) = s
        .begin_verification(r.id, a.id, "file", None)
        .await
        .unwrap();
    s.finish_verification(&id, "PASS", json!({"readable":true}))
        .await
        .unwrap();
    assert!(s.complete_run(r.id, a.id, result.clone()).await.is_err());
    s.submit_human_review(
        t.id,
        r.id,
        "human",
        "PASS",
        "Operator inspected delivery",
        "human:operator",
        t.current_context_revision_id,
        t.acceptance_version,
    )
    .await
    .unwrap();
    assert!(
        s.run_completion_check(r.id, a.id, &result)
            .await
            .unwrap()
            .ready
    );
}

#[test]
fn rejects_ambiguous_or_unsafe_contracts() {
    let c = criterion("check", "deterministic_check");
    assert!(validate_acceptance(&[c.clone(), c.clone()]).is_err());
    let mut c = c;
    c.check.as_mut().unwrap().read_only = false;
    assert!(validate_acceptance(&[c]).is_err());
    let mut c = criterion("file", "artifact_check");
    c.artifact_check.as_mut().unwrap().path = "relative".into();
    assert!(validate_acceptance(&[c]).is_err());
}

#[tokio::test]
async fn review_snapshot_and_evidence_must_match_and_unknown_attempts_need_reconciliation() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    let a = s.register_agent("executor", &[]).await.unwrap();
    let b = s.register_agent("reviewer", &[]).await.unwrap();
    let t = task(
        &s,
        vec![
            criterion("review", "independent_review"),
            criterion("test", "deterministic_check"),
            criterion("human", "human_review"),
        ],
    )
    .await;
    let r = run(&s, &t, a.id, "executor").await;
    let rr = run(&s, &t, b.id, "reviewer").await;
    let (result, artifact) = report(&s, &t, &r, a.id).await;
    assert!(
        s.submit_review(
            rr.id,
            b.id,
            r.id,
            "review",
            "PASS",
            "reviewed",
            &[artifact.clone()],
            Some(uuid::Uuid::new_v4()),
            t.acceptance_version,
            None
        )
        .await
        .is_err()
    );
    assert!(
        s.submit_human_review(
            t.id,
            r.id,
            "human",
            "PASS",
            "approved",
            "human:operator",
            Some(uuid::Uuid::new_v4()),
            t.acceptance_version
        )
        .await
        .is_err()
    );
    let (id, _) = s
        .begin_verification(r.id, a.id, "test", None)
        .await
        .unwrap();
    s.finish_verification(&id, "BLOCKED", json!({"transport":"lost"}))
        .await
        .unwrap();
    assert!(
        s.begin_verification(r.id, a.id, "test", Some("try again"))
            .await
            .is_err()
    );
    s.reconcile_verification(
        t.id,
        &id,
        "Confirmed the first command failed; no external effect observed",
        json!({"label":"operator"}),
    )
    .await
    .unwrap();
    assert!(
        s.reconcile_verification(t.id, &id, "again", json!({}))
            .await
            .is_err()
    );
    let (id, _) = s
        .begin_verification(
            r.id,
            a.id,
            "test",
            Some("Environment repaired after reconciliation"),
        )
        .await
        .unwrap();
    s.finish_verification(&id, "PASS", json!({"exit_code":0}))
        .await
        .unwrap();
    s.submit_human_review(
        t.id,
        r.id,
        "human",
        "PASS",
        "approved",
        "human:actual-operator",
        t.current_context_revision_id,
        t.acceptance_version,
    )
    .await
    .unwrap();
    s.submit_review(
        rr.id,
        b.id,
        r.id,
        "review",
        "PASS",
        "reviewed",
        &[artifact],
        t.current_context_revision_id,
        t.acceptance_version,
        None,
    )
    .await
    .unwrap();
    assert!(
        s.run_completion_check(r.id, a.id, &result)
            .await
            .unwrap()
            .ready
    );
    let (different, _) = report(&s, &t, &r, a.id).await;
    assert!(
        !s.run_completion_check(r.id, a.id, &different)
            .await
            .unwrap()
            .ready
    );
    let (id, _) = s
        .begin_verification(
            r.id,
            a.id,
            "test",
            Some("Rechecked after implementation changes"),
        )
        .await
        .unwrap();
    s.finish_verification(&id, "FAIL", json!({"exit_code":1}))
        .await
        .unwrap();
    assert!(s.complete_run(r.id, a.id, result).await.is_err());
}

#[tokio::test]
async fn optional_runtime_waiver_can_complete_with_independent_review() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    let a = s.register_agent("executor", &[]).await.unwrap();
    let b = s.register_agent("reviewer", &[]).await.unwrap();
    let mut optional = criterion("optional", "deterministic_check");
    optional.allow_not_applicable = true;
    let t = task(
        &s,
        vec![optional, criterion("review", "independent_review")],
    )
    .await;
    let r = run(&s, &t, a.id, "executor").await;
    let rr = run(&s, &t, b.id, "reviewer").await;
    let (mut result, artifact) = report(&s, &t, &r, a.id).await;
    result["completion"]["checks"][0]["status"] = json!("NOT_APPLICABLE");
    result["completion"]["checks"][0]["rationale"] =
        json!("Target capability is not part of this delivery");
    result["completion"]["checks"][0]["artifact_ids"] = json!([]);
    s.submit_review(
        rr.id,
        b.id,
        r.id,
        "review",
        "PASS",
        "Reviewed the delivered scope and allowed waiver",
        &[artifact],
        t.current_context_revision_id,
        t.acceptance_version,
        None,
    )
    .await
    .unwrap();
    assert!(
        s.run_completion_check(r.id, a.id, &result)
            .await
            .unwrap()
            .ready
    );
}
