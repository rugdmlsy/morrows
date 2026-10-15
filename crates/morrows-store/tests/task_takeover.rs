use morrows_store::Store;
use serde_json::json;
use sqlx::SqlitePool;
use uuid::Uuid;

async fn fixture() -> (Store, SqlitePool, Uuid, Uuid, Uuid, morrows_core::TaskClaim) {
    let name = format!("/tmp/morrows-takeover-{}.db", Uuid::new_v4());
    let db = format!("sqlite://{name}");
    let store = Store::connect(&db).await.unwrap();
    let pool = SqlitePool::connect(&db).await.unwrap();
    let a = store
        .register_agent(&format!("old-{}", Uuid::new_v4()), &[])
        .await
        .unwrap();
    let b = store
        .register_agent(&format!("new-{}", Uuid::new_v4()), &[])
        .await
        .unwrap();
    let project = store
        .create_project(serde_json::from_value(json!({"name":"fixture"})).unwrap())
        .await
        .unwrap();
    let task = store
        .create_task(
            serde_json::from_value(json!({"title":"legacy no-context","project_id":project.id}))
                .unwrap(),
        )
        .await
        .unwrap();
    let prior = store
        .claim_task_for_execution(task.id, a.id, "executor", 900)
        .await
        .unwrap();
    (store, pool, task.id, a.id, b.id, prior)
}

async fn expire(pool: &SqlitePool, assignment: Uuid) {
    sqlx::query("UPDATE assignments SET expires_at='2020-01-01T00:00:00Z' WHERE id=?")
        .bind(assignment.to_string())
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn expired_cross_agent_takeover_preserves_contextless_history_and_fences_old_owner() {
    let (store, pool, task, a, b, prior) = fixture().await;
    assert_eq!(prior.run.context_revision_id, None);
    assert!(
        !store.task_takeover_status(task).await.unwrap()["eligible"]
            .as_bool()
            .unwrap()
    );
    assert!(
        store
            .takeover_task(task, b, 900, "not expired", false, None)
            .await
            .is_err()
    );
    store
        .checkpoint_run(prior.run.id, a, json!({"milestone":"keep me"}))
        .await
        .unwrap();
    expire(&pool, prior.assignment.id).await;
    assert!(
        store.task_takeover_status(task).await.unwrap()["eligible"]
            .as_bool()
            .unwrap()
    );
    let next = store
        .takeover_task(task, b, 900, "lost original owner", false, None)
        .await
        .unwrap();
    assert_ne!(next.assignment.id, prior.assignment.id);
    assert_ne!(next.run.id, prior.run.id);
    assert_eq!(next.assignment.phase, "context_review");
    assert_eq!(next.run.context_revision_id, None);
    assert_eq!(
        store.get_task(task).await.unwrap().state.to_string(),
        "in_progress"
    );
    assert_eq!(
        store
            .get_assignment(prior.assignment.id)
            .await
            .unwrap()
            .status,
        "released"
    );
    let old = store.get_run(prior.run.id).await.unwrap();
    assert_eq!(old.status, "abandoned");
    assert_eq!(old.checkpoint.unwrap()["milestone"], "keep me");
    assert!(
        store
            .renew_assignment(prior.assignment.id, a, 900)
            .await
            .is_err()
    );
    assert!(
        store
            .complete_run(prior.run.id, a, json!({}))
            .await
            .is_err()
    );
    assert!(
        store
            .checkpoint_run(prior.run.id, a, json!({}))
            .await
            .is_err()
    );
    assert!(
        store
            .authorize_run_runtime_access(prior.run.id, a)
            .await
            .is_err()
    );
    assert!(store.task_takeover_status(task).await.unwrap()["eligible"] == false);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM assignments WHERE task_id=? AND role='executor' AND status='active'",
    )
    .bind(task.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn concurrent_takeover_is_cas_only_one_wins() {
    let (store, pool, task, _a, b, prior) = fixture().await;
    let c = store
        .register_agent(&format!("racer-{}", Uuid::new_v4()), &[])
        .await
        .unwrap();
    expire(&pool, prior.assignment.id).await;
    let x = store.clone();
    let y = store.clone();
    let (r1, r2) = tokio::join!(
        async move { x.takeover_task(task, b, 900, "racer b", false, None).await },
        async move {
            y.takeover_task(task, c.id, 900, "racer c", false, None)
                .await
        }
    );
    assert_ne!(r1.is_ok(), r2.is_ok());
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM assignments WHERE task_id=? AND role='executor' AND status='active'",
    )
    .bind(task.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn force_requires_operator_and_cannot_bypass_cleanup() {
    let (store, pool, task, _a, b, prior) = fixture().await;
    assert!(
        store
            .takeover_task(task, b, 900, "force", true, None)
            .await
            .is_err()
    );
    assert!(
        store
            .takeover_task(task, b, 900, "normal", false, None)
            .await
            .is_err()
    );
    sqlx::query("UPDATE runs SET status='cleanup_pending' WHERE id=?")
        .bind(prior.run.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store
            .takeover_task(task, b, 900, "operator force", true, Some("operator-admin"))
            .await
            .is_err()
    );
    sqlx::query("UPDATE runs SET status='running' WHERE id=?")
        .bind(prior.run.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    let next = store
        .takeover_task(
            task,
            b,
            900,
            "operator authorized force",
            true,
            Some("operator-admin"),
        )
        .await
        .unwrap();
    assert_eq!(next.assignment.agent_instance_id, b);
    assert_eq!(
        store.get_run(prior.run.id).await.unwrap().status,
        "abandoned"
    );
}

#[tokio::test]
async fn approval_task_never_implicitly_becomes_open() {
    let (store, pool, task, _a, b, prior) = fixture().await;
    expire(&pool, prior.assignment.id).await;
    store
        .set_task_assignment_mode(task, "approval".parse().unwrap())
        .await
        .unwrap();
    assert!(
        store
            .takeover_task(task, b, 900, "ordinary", false, None)
            .await
            .is_err()
    );
    let status = store.task_takeover_status(task).await.unwrap();
    assert_eq!(status["employee_self_takeover"], false);
    let transferred = store
        .takeover_task(task, b, 900, "admin assigned", true, Some("admin"))
        .await
        .unwrap();
    assert_eq!(transferred.assignment.agent_instance_id, b);
    assert_eq!(
        store
            .get_task(task)
            .await
            .unwrap()
            .assignment_mode
            .to_string(),
        "approval"
    );
}

#[tokio::test]
async fn idempotent_retry_returns_original_receipt_without_duplicate_execution() {
    let (store, pool, task, _a, b, prior) = fixture().await;
    expire(&pool, prior.assignment.id).await;
    let req = "external-provider-attempt-0001";
    let first = store
        .takeover_task_request(task, b, 900, "retry safe", false, None, Some(req))
        .await
        .unwrap();
    let replay = store
        .takeover_task_request(task, b, 900, "retry safe", false, None, Some(req))
        .await
        .unwrap();
    assert_eq!(first.run.id, replay.run.id);
    assert_eq!(first.assignment.id, replay.assignment.id);
    assert!(
        store
            .takeover_task_request(task, b, 900, "different purpose", false, None, Some(req))
            .await
            .is_err()
    );
    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE entity_id=? AND event_type='task.executor_taken_over'",
    )
    .bind(task.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(events, 1);
}

#[tokio::test]
async fn orphan_in_progress_task_without_any_executor_can_be_recovered() {
    let (store, pool, task, _a, b, prior) = fixture().await;
    sqlx::query("DELETE FROM run_context_revisions WHERE run_id=?")
        .bind(prior.run.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM runs WHERE id=?")
        .bind(prior.run.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM assignments WHERE id=?")
        .bind(prior.assignment.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.task_takeover_status(task).await.unwrap()["eligible"] == true);
    let next = store
        .takeover_task(task, b, 900, "legacy orphan", false, None)
        .await
        .unwrap();
    assert_eq!(next.assignment.phase, "context_review");
    assert_eq!(next.assignment.agent_instance_id, b);
}

#[tokio::test]
async fn runtime_binding_blocks_takeover_even_after_lease_expiry() {
    let (store, pool, task, _a, b, prior) = fixture().await;
    expire(&pool, prior.assignment.id).await;
    sqlx::query("INSERT INTO run_runtime_bindings(run_id,runtime_scope_id,created_at,updated_at) VALUES(?,?,?,?)")
        .bind(prior.run.id.to_string()).bind("isolated-active-scope")
        .bind("2026-10-15T00:00:00Z").bind("2026-10-15T00:00:00Z")
        .execute(&pool).await.unwrap();
    let status = store.task_takeover_status(task).await.unwrap();
    assert_eq!(status["eligible"], false);
    assert!(
        store
            .takeover_task(task, b, 900, "not quiesced", true, Some("admin"))
            .await
            .is_err()
    );
    sqlx::query("UPDATE run_runtime_bindings SET scope_terminalized_at='2026-10-15T00:00:00Z' WHERE run_id=?")
        .bind(prior.run.id.to_string()).execute(&pool).await.unwrap();
    assert!(
        store
            .takeover_task(task, b, 900, "quiesced", false, None)
            .await
            .is_ok()
    );
}
