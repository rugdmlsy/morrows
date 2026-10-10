use morrows_core::*;
use morrows_store::Store;
use serde_json::{Value, json};

fn criterion(id: &str, review: bool) -> AcceptanceCriterion {
    serde_json::from_value(json!({
        "id":id,"requirement":format!("Verify {id}"),
        "verification":if review {"independent_review"} else {"self_attested"}
    }))
    .unwrap()
}
async fn result_with_evidence(s: &Store, task_id: Id, run_id: Id, agent: Id) -> Value {
    let artifact = s
        .create_artifact(
            task_id,
            agent,
            CreateArtifact {
                title: "Revision evidence".into(),
                uri: "/tmp/revised-report".into(),
                kind: "report".into(),
                description: "Revised test evidence".into(),
            },
        )
        .await
        .unwrap();
    let mut template = s
        .run_completion_check(run_id, agent, &json!({}))
        .await
        .unwrap()
        .completion_template;
    for check in template["checks"].as_array_mut().unwrap() {
        check["status"] = json!("PASS");
        check["rationale"] = json!("Checked revised criterion with same-task artifact");
        check["artifact_ids"] = json!([artifact.id]);
    }
    json!({"completion":template})
}
#[tokio::test]
async fn accepted_revision_requires_fresh_independent_review_then_finishes_same_run() {
    let s = Store::connect("sqlite::memory:").await.unwrap();
    let executor = s.register_agent("revised-executor", &[]).await.unwrap();
    let reviewer = s.register_agent("revised-reviewer", &[]).await.unwrap();
    let t = s
        .create_task(CreateTask {
            title: "Revised task".into(),
            description: "Old".into(),
            project_id: None,
            owner_actor_id: "human:fixture".into(),
            state: TaskState::Ready,
            priority: 0,
            acceptance_criteria: vec![criterion("review", false)],
        })
        .await
        .unwrap();
    let ea = s
        .claim_task(t.id, executor.id, "executor", 300)
        .await
        .unwrap();
    let run = s.start_run(ea.id, executor.id, None).await.unwrap();
    let ra = s
        .claim_task(t.id, reviewer.id, "reviewer", 300)
        .await
        .unwrap();
    let reviewer_run = s.start_run(ra.id, reviewer.id, None).await.unwrap();
    let older = result_with_evidence(&s, t.id, run.id, executor.id).await;
    assert!(
        s.run_completion_check(run.id, executor.id, &older)
            .await
            .unwrap()
            .ready
    );
    let proposed = s
        .propose_task_revision(
            t.id,
            "human:fixture",
            TaskRevisionDraft {
                expected_version: 1,
                reason: "Reviewer needed for stronger acceptance".into(),
                title: None,
                description: Some("Updated".into()),
                goal: Some("Review revised evidence".into()),
                scope: None,
                acceptance_criteria: Some(vec![criterion("review", true)]),
            },
        )
        .await
        .unwrap();
    assert!(
        !s.run_completion_check(run.id, executor.id, &older)
            .await
            .unwrap()
            .ready
    );
    assert!(
        s.complete_run(run.id, executor.id, older.clone())
            .await
            .is_err()
    );
    s.ack_task_revision(
        t.id,
        executor.id,
        TaskRevisionAck {
            revision_id: proposed["revisions"][0]["id"].as_str().unwrap().into(),
            run_id: run.id.to_string(),
            impact: "Fresh independent review required; previous attestation insufficient".into(),
            updated_plan: vec![
                "Produce refreshed artifact".into(),
                "Request independent review".into(),
            ],
        },
    )
    .await
    .unwrap();
    let updated = s.get_task(t.id).await.unwrap();
    assert_eq!(updated.acceptance_version, 2);
    assert_eq!(s.task_runs(t.id).await.unwrap().len(), 2);
    assert!(
        s.run_completion_check(run.id, executor.id, &older)
            .await
            .unwrap()
            .ready
            == false
    );
    let revised = result_with_evidence(&s, t.id, run.id, executor.id).await;
    assert!(
        !s.run_completion_check(run.id, executor.id, &revised)
            .await
            .unwrap()
            .ready
    );
    let evidence_id = revised["completion"]["checks"][0]["artifact_ids"][0]
        .as_str()
        .unwrap()
        .to_string();
    s.submit_review(
        reviewer_run.id,
        reviewer.id,
        run.id,
        "review",
        "PASS",
        "Reviewed new contract",
        &[evidence_id],
        updated.current_context_revision_id,
        updated.acceptance_version,
        None,
    )
    .await
    .unwrap();
    assert!(
        s.run_completion_check(run.id, executor.id, &revised)
            .await
            .unwrap()
            .ready
    );
    s.complete_run(run.id, executor.id, revised).await.unwrap();
    let done = s.get_task(t.id).await.unwrap();
    assert_eq!(done.state, TaskState::Done);
    let records = s.completion_records(t.id).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["acceptance_version"], 2);
    assert_eq!(
        records[0]["criteria_snapshot"][0]["verification"],
        "independent_review"
    );
    assert_eq!(records[0]["criterion_verdicts"][0]["verdict"], "PASS");
    assert!(
        s.propose_task_revision(
            t.id,
            "human:fixture",
            TaskRevisionDraft {
                expected_version: 2,
                reason: "No rewrite".into(),
                title: None,
                description: Some("Changed".into()),
                goal: None,
                scope: None,
                acceptance_criteria: None
            }
        )
        .await
        .is_err()
    );
}
