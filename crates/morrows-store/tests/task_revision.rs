use morrows_core::*;
use morrows_store::Store;
use serde_json::json;

fn criterion(id: &str) -> AcceptanceCriterion {
    serde_json::from_value(json!({"id":id,"requirement":format!("Show evidence for {id}"),"verification":"self_attested"})).unwrap()
}
async fn task(store: &Store, title: &str) -> Task {
    store
        .create_task(CreateTask {
            title: title.into(),
            description: "old description".into(),
            owner_actor_id: "human:test".into(),
            acceptance_criteria: vec![criterion("old")],
            project_id: None,
            state: TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap()
}
fn draft(version: i64) -> TaskRevisionDraft {
    TaskRevisionDraft {
        expected_version: version,
        reason: "Evidence-driven scope correction".into(),
        title: None,
        description: Some("new description".into()),
        goal: Some("new objective".into()),
        scope: Some("evidence-driven gap validation".into()),
        acceptance_criteria: Some(vec![criterion("new")]),
    }
}
#[tokio::test]
async fn ready_revision_is_atomic_historical_and_cas_protected() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let t = task(&store, "baseline").await;
    let old_context = t.current_context_revision_id;
    let initial = store.task_revision_history(t.id).await.unwrap();
    assert_eq!(initial["active_version"], 1);
    assert_eq!(
        initial["revisions"][0]["after"]["acceptance_criteria"][0]["id"],
        "old"
    );
    let preview = store.preview_task_revision(t.id, draft(1)).await.unwrap();
    assert_eq!(preview["before"]["description"], "old description");
    assert_eq!(preview["after"]["description"], "new description");
    assert_eq!(
        store.get_task(t.id).await.unwrap().description,
        "old description"
    );
    let applied = store
        .propose_task_revision(t.id, "human:test", draft(1))
        .await
        .unwrap();
    assert_eq!(applied["active_version"], 2);
    assert_eq!(applied["acceptance_version"], 2);
    assert_eq!(applied["revisions"][0]["status"], "applied");
    assert_eq!(
        applied["revisions"][0]["diff"]["acceptance_criteria"]["before"][0]["id"],
        "old"
    );
    assert_eq!(
        applied["revisions"][1]["after"]["acceptance_criteria"][0]["id"],
        "old"
    );
    let updated = store.get_task(t.id).await.unwrap();
    assert_ne!(updated.current_context_revision_id, old_context);
    assert_eq!(updated.acceptance_criteria[0].id, "new");
    let ctx = store.get_current_context(t.id).await.unwrap();
    assert_eq!(ctx.goal, "new objective");
    assert_eq!(
        ctx.constraints["task_scope"],
        "evidence-driven gap validation"
    );
    assert!(
        store
            .propose_task_revision(t.id, "human:test", draft(1))
            .await
            .unwrap_err()
            .to_string()
            .contains("stale")
    );
    assert!(
        store
            .propose_task_revision(t.id, "agent:untrusted", draft(2))
            .await
            .unwrap_err()
            .to_string()
            .contains("publisher")
    );
    let cancelled = store.cancel_task(t.id).await.unwrap();
    assert_eq!(cancelled.state, TaskState::Cancelled);
    assert!(
        store
            .propose_task_revision(t.id, "human:test", draft(2))
            .await
            .unwrap_err()
            .to_string()
            .contains("rework")
    );
}
#[tokio::test]
async fn running_revision_blocks_completion_until_explicit_executor_replan() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let t = task(&store, "live").await;
    let executor = store
        .register_agent("revision-executor", &[])
        .await
        .unwrap();
    let unauthorized = store
        .register_agent("revision-unauthorized", &[])
        .await
        .unwrap();
    let assignment = store
        .claim_task(t.id, executor.id, "executor", 1200)
        .await
        .unwrap();
    let run = store
        .start_run(assignment.id, executor.id, None)
        .await
        .unwrap();
    let pending = store
        .propose_task_revision(t.id, "human:test", draft(1))
        .await
        .unwrap();
    assert_eq!(pending["revisions"][0]["status"], "pending_ack");
    assert_eq!(pending["active_version"], 1);
    assert_eq!(
        store.get_task(t.id).await.unwrap().acceptance_criteria[0].id,
        "old"
    );
    assert!(
        !store
            .agent_delivery_inbox(executor.id, 20)
            .await
            .unwrap()
            .is_empty()
    );
    let readiness = store
        .run_completion_check(run.id, executor.id, &json!({}))
        .await
        .unwrap();
    assert!(!readiness.ready);
    assert!(readiness.blockers.iter().any(|b| b.contains("revision")));
    assert!(
        store
            .propose_task_revision(t.id, "human:test", draft(1))
            .await
            .unwrap_err()
            .to_string()
            .contains("pending")
    );
    assert!(
        store
            .propose_task_revision(t.id, &format!("agent:{}", executor.id), draft(1))
            .await
            .unwrap_err()
            .to_string()
            .contains("publisher")
    );
    let rev = pending["revisions"][0]["id"].as_str().unwrap().to_owned();
    let mut ack = TaskRevisionAck {
        revision_id: rev,
        run_id: run.id.to_string(),
        impact: "Older 36 checks no longer match evidence gap".into(),
        updated_plan: vec!["Collect missing evidence".into(), "Run new criteria".into()],
    };
    assert!(
        store
            .ack_task_revision(t.id, unauthorized.id, ack.clone())
            .await
            .is_err()
    );
    ack.updated_plan.clear();
    assert!(
        store
            .ack_task_revision(t.id, executor.id, ack.clone())
            .await
            .unwrap_err()
            .to_string()
            .contains("updated_plan")
    );
    ack.updated_plan.push("Execute revised checks".into());
    let result = store
        .ack_task_revision(t.id, executor.id, ack)
        .await
        .unwrap();
    assert_eq!(result["active_version"], 2);
    assert_eq!(result["revisions"][0]["status"], "applied");
    assert_eq!(
        result["revisions"][0]["ack_agent_id"],
        executor.id.to_string()
    );
    assert_eq!(
        result["revisions"][0]["updated_plan"][0],
        "Execute revised checks"
    );
    assert_eq!(
        store.get_task(t.id).await.unwrap().acceptance_criteria[0].id,
        "new"
    );
    assert_eq!(store.task_runs(t.id).await.unwrap().len(), 1);
    assert!(
        store
            .ack_task_revision(
                t.id,
                executor.id,
                TaskRevisionAck {
                    revision_id: result["revisions"][0]["id"].as_str().unwrap().into(),
                    run_id: run.id.to_string(),
                    impact: "again".into(),
                    updated_plan: vec!["again".into()]
                }
            )
            .await
            .is_err()
    );
}
#[tokio::test]
async fn rejection_and_cancel_preserve_prior_effective_contract() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let t = task(&store, "rejection").await;
    let executor = store.register_agent("reject-executor", &[]).await.unwrap();
    let assignment = store
        .claim_task(t.id, executor.id, "executor", 1200)
        .await
        .unwrap();
    let _run = store
        .start_run(assignment.id, executor.id, None)
        .await
        .unwrap();
    let history = store
        .propose_task_revision(t.id, "human:test", draft(1))
        .await
        .unwrap();
    let id = uuid::Uuid::parse_str(history["revisions"][0]["id"].as_str().unwrap()).unwrap();
    let rejected = store
        .reject_task_revision(t.id, "human:test", id, "Spec needs revision")
        .await
        .unwrap();
    assert_eq!(rejected["active_version"], 1);
    assert_eq!(rejected["revisions"][0]["status"], "rejected");
    assert_eq!(
        store.get_task(t.id).await.unwrap().acceptance_criteria[0].id,
        "old"
    );
    let second = store
        .propose_task_revision(t.id, "human:test", draft(1))
        .await
        .unwrap();
    assert_eq!(second["revisions"][0]["status"], "pending_ack");
    store.cancel_task(t.id).await.unwrap();
    let cancelled = store.task_revision_history(t.id).await.unwrap();
    assert_eq!(cancelled["revisions"][0]["status"], "rejected");
    assert_eq!(cancelled["active_version"], 1);
}
#[tokio::test]
async fn overlapping_proposals_have_one_cas_winner() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let t = task(&store, "race").await;
    let (a, b) = tokio::join!(
        store.propose_task_revision(t.id, "human:test", draft(1)),
        store.propose_task_revision(t.id, "human:test", draft(1))
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(
        store.task_revision_history(t.id).await.unwrap()["revisions"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn old_pass_review_receipt_is_historical_not_fresh_acceptance() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let t=store.create_task(CreateTask{
       title:"Review receipt".into(),description:"original".into(),owner_actor_id:"human:test".into(),
       acceptance_criteria:vec![serde_json::from_value(json!({"id":"review","requirement":"Old human check","verification":"human_review"})).unwrap()],
       project_id:None,state:TaskState::Ready,priority:0
    }).await.unwrap();
    let agent = store.register_agent("review-revision", &[]).await.unwrap();
    let assignment = store
        .claim_task(t.id, agent.id, "executor", 1200)
        .await
        .unwrap();
    let run = store
        .start_run(assignment.id, agent.id, None)
        .await
        .unwrap();
    store
        .submit_human_review(
            t.id,
            run.id,
            "review",
            "PASS",
            "Old review passed",
            "human:reviewer",
            None,
            1,
        )
        .await
        .unwrap();
    let receipts = store.verification_receipts(t.id).await.unwrap();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["verdict"], "PASS");
    assert_eq!(receipts[0]["acceptance_version"], 1);
    let proposed=store.propose_task_revision(t.id,"human:test",TaskRevisionDraft{
       expected_version:1,reason:"Strengthen acceptance".into(),title:None,description:None,goal:None,scope:None,
       acceptance_criteria:Some(vec![serde_json::from_value(json!({"id":"review","requirement":"New human check","verification":"human_review"})).unwrap()])
    }).await.unwrap();
    let rev = proposed["revisions"][0]["id"].as_str().unwrap();
    assert_eq!(proposed["revisions"][0]["status"], "pending_ack");
    assert!(
        store
            .submit_human_review(
                t.id,
                run.id,
                "review",
                "PASS",
                "try after pending",
                "human:reviewer",
                None,
                1
            )
            .await
            .is_err()
    );
    store
        .ack_task_revision(
            t.id,
            agent.id,
            TaskRevisionAck {
                revision_id: rev.to_owned(),
                run_id: run.id.to_string(),
                impact: "Need a fresh human review".into(),
                updated_plan: vec!["Re-review".into()],
            },
        )
        .await
        .unwrap();
    assert_eq!(store.get_task(t.id).await.unwrap().acceptance_version, 2);
    let status = store
        .run_completion_check(run.id, agent.id, &json!({}))
        .await
        .unwrap();
    let check = status
        .criterion_statuses
        .iter()
        .find(|c| c.criterion_id == "review")
        .unwrap();
    assert_ne!(check.verdict, "PASS");
    assert!(check.receipt_ids.is_empty(), "old PASS must not be reused");
    let old = store.verification_receipts(t.id).await.unwrap();
    assert_eq!(old.len(), 1);
    assert_eq!(old[0]["acceptance_version"], 1);
}

#[tokio::test]
async fn an_offline_executor_retains_pending_revision_notification() {
    let db = std::env::temp_dir().join(format!(
        "morrows-revision-offline-{}.db",
        uuid::Uuid::new_v4()
    ));
    let url = format!("sqlite://{}", db.display());
    let store = Store::connect(&url).await.unwrap();
    let t = task(&store, "offline").await;
    let agent = store.register_agent("offline-revision", &[]).await.unwrap();
    let assignment = store
        .claim_task(t.id, agent.id, "executor", 1)
        .await
        .unwrap();
    let run = store
        .start_run(assignment.id, agent.id, None)
        .await
        .unwrap();
    let before = store
        .propose_task_revision(t.id, "human:test", draft(1))
        .await
        .unwrap();
    assert_eq!(before["revisions"][0]["status"], "pending_ack");
    // Simulate a truly expired/offline Assignment without a slow 30-second lease wait.
    let sql = sqlx::SqlitePool::connect(&url).await.unwrap();
    sqlx::query("UPDATE assignments SET expires_at='2000-01-01T00:00:00+00:00' WHERE id=?")
        .bind(assignment.id.to_string())
        .execute(&sql)
        .await
        .unwrap();
    assert_eq!(store.expire_stale_assignments().await.unwrap(), 1);
    assert!(
        !store
            .agent_delivery_inbox(agent.id, 10)
            .await
            .unwrap()
            .is_empty()
    );
    let ack = TaskRevisionAck {
        revision_id: before["revisions"][0]["id"].as_str().unwrap().into(),
        run_id: run.id.to_string(),
        impact: "recovered".into(),
        updated_plan: vec!["Continue".into()],
    };
    assert!(
        store
            .ack_task_revision(t.id, agent.id, ack)
            .await
            .unwrap_err()
            .to_string()
            .contains("active implementing")
    );
    assert_eq!(
        store.task_revision_history(t.id).await.unwrap()["revisions"][0]["status"],
        "pending_ack"
    );
}

#[tokio::test]
async fn context_changed_during_pending_requires_reproposal() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let t = task(&store, "changing").await;
    let worker = store.register_agent("context-changing", &[]).await.unwrap();
    let a = store
        .claim_task(t.id, worker.id, "executor", 1200)
        .await
        .unwrap();
    let run = store.start_run(a.id, worker.id, None).await.unwrap();
    let pending = store
        .propose_task_revision(t.id, "human:test", draft(1))
        .await
        .unwrap();
    store
        .create_context_revision(
            t.id,
            CreateContextRevision {
                goal: "Externally changed".into(),
                background: String::new(),
                constraints: json!({}),
                current_summary: String::new(),
                created_by_actor_id: "human:test".into(),
            },
        )
        .await
        .unwrap();
    let err = store
        .ack_task_revision(
            t.id,
            worker.id,
            TaskRevisionAck {
                revision_id: pending["revisions"][0]["id"].as_str().unwrap().into(),
                run_id: run.id.to_string(),
                impact: "stale".into(),
                updated_plan: vec!["no".into()],
            },
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("context changed"));
}
