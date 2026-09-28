use morrows_core::*;
use morrows_store::Store;
use serde_json::json;
use uuid::Uuid;

async fn setup() -> (Store, String, AgentInstance, Task, Assignment, Run) {
    let path = std::env::temp_dir().join(format!("morrows-recovery-{}.db", Uuid::new_v4()));
    let url = format!("sqlite://{}", path.display());
    let store = Store::connect(&url).await.unwrap();
    let agent = store.register_agent("worker", &[]).await.unwrap();
    let task = store
        .create_task(serde_json::from_value(json!({"title":"recover me"})).unwrap())
        .await
        .unwrap();
    let assignment = store
        .claim_task(task.id, agent.id, "executor", 300)
        .await
        .unwrap();
    let run = store
        .start_run(assignment.id, agent.id, None)
        .await
        .unwrap();
    (store, url, agent, task, assignment, run)
}

async fn expire(store: &Store, url: &str, assignment: Id) {
    let pool = sqlx::SqlitePool::connect(url).await.unwrap();
    sqlx::query("UPDATE assignments SET expires_at='2000-01-01T00:00:00Z' WHERE id=?")
        .bind(assignment.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(store.expire_stale_assignments().await.unwrap(), 1);
    pool.close().await;
}

fn milestone() -> CreateRunMilestone {
    CreateRunMilestone {
        kind: "milestone".into(),
        summary: "recovered execution".into(),
        completed: vec!["recovered original lease".into()],
        verified: vec!["same run remains writable".into()],
        remaining: vec!["complete".into()],
        blockers: vec![],
        next_step: "complete run".into(),
        next_plan: vec!["checkpoint".into(), "complete run".into()],
        execution_locations: vec!["test".into()],
        artifact_ids: vec![],
        decision_ids: vec![],
    }
}

#[tokio::test]
async fn expired_assignment_recovers_same_run_and_can_finish() {
    let (store, url, agent, task, assignment, run) = setup().await;
    expire(&store, &url, assignment.id).await;

    let recovered = store
        .recover_assignment(
            assignment.id,
            run.id,
            agent.id,
            300,
            "resume after long-running lease expiry",
        )
        .await
        .unwrap();
    assert_eq!(recovered.assignment.id, assignment.id);
    assert_eq!(recovered.run.id, run.id);
    assert_eq!(recovered.assignment.status, "active");
    assert_eq!(store.task_runs(task.id).await.unwrap().len(), 1);

    store
        .create_run_milestone(run.id, agent.id, milestone())
        .await
        .unwrap();
    store
        .checkpoint_run(run.id, agent.id, json!({"recovered":true}))
        .await
        .unwrap();
    store
        .complete_run(run.id, agent.id, json!({"ok":true}))
        .await
        .unwrap();
    assert_eq!(store.get_run(run.id).await.unwrap().status, "completed");

    let events = store.task_events(task.id).await.unwrap();
    let recovered_event = events
        .into_iter()
        .find(|event| event.event_type == "assignment.recovered")
        .unwrap();
    assert_eq!(
        recovered_event.payload["assignment_id"],
        assignment.id.to_string()
    );
    assert_eq!(recovered_event.payload["run_id"], run.id.to_string());
    assert_eq!(
        recovered_event.payload["reason"],
        "resume after long-running lease expiry"
    );
}

#[tokio::test]
async fn concurrent_recovery_has_exactly_one_winner() {
    let (store, url, agent, _task, assignment, run) = setup().await;
    expire(&store, &url, assignment.id).await;
    let left = store.clone();
    let right = store.clone();
    let (one, two) = tokio::join!(
        left.recover_assignment(assignment.id, run.id, agent.id, 300, "left"),
        right.recover_assignment(assignment.id, run.id, agent.id, 300, "right")
    );
    assert_ne!(one.is_ok(), two.is_ok());
}

#[tokio::test]
async fn recovery_rejects_another_active_executor() {
    let (store, url, agent, task, assignment, run) = setup().await;
    expire(&store, &url, assignment.id).await;
    let other = store.register_agent("other", &[]).await.unwrap();
    let active = store
        .claim_task(task.id, other.id, "executor", 300)
        .await
        .unwrap();
    let error = store
        .recover_assignment(assignment.id, run.id, agent.id, 300, "should fail")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("another active executor"));
    assert_eq!(
        store.get_assignment(active.id).await.unwrap().status,
        "active"
    );
}

#[tokio::test]
async fn recovery_rejects_terminal_run_and_terminal_task() {
    let (store, url, agent, task, assignment, run) = setup().await;
    expire(&store, &url, assignment.id).await;
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();

    sqlx::query("UPDATE runs SET status='completed',ended_at='2000-01-01T00:00:01Z' WHERE id=?")
        .bind(run.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let error = store
        .recover_assignment(assignment.id, run.id, agent.id, 300, "terminal run")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("only running or paused"));

    sqlx::query("UPDATE runs SET status='running',ended_at=NULL WHERE id=?")
        .bind(run.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE tasks SET state='done' WHERE id=?")
        .bind(task.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let error = store
        .recover_assignment(assignment.id, run.id, agent.id, 300, "terminal task")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("requires in_progress"));

    sqlx::query("UPDATE tasks SET state='cancelled' WHERE id=?")
        .bind(task.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let error = store
        .recover_assignment(assignment.id, run.id, agent.id, 300, "cancelled task")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("requires in_progress"));

    pool.close().await;
}

#[tokio::test]
async fn recovery_rejects_wrong_agent_and_live_lease() {
    let (store, _url, agent, _task, assignment, run) = setup().await;
    let other = store.register_agent("other", &[]).await.unwrap();

    let error = store
        .recover_assignment(assignment.id, run.id, other.id, 300, "steal")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("another agent"));

    let error = store
        .recover_assignment(assignment.id, run.id, agent.id, 300, "too early")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("use assignment_renew"));
}
