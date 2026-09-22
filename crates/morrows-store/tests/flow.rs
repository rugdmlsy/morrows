use morrows_core::{CreateContextRevision, CreateTask, DomainError, TaskState};
use morrows_store::Store;
use serde_json::json;
use std::path::PathBuf;
use uuid::Uuid;

fn test_db() -> (String, PathBuf) {
    let path = std::env::temp_dir().join(format!("morrows-{}.db", Uuid::new_v4()));
    (format!("sqlite://{}", path.display()), path)
}

#[tokio::test]
async fn vertical_slice_survives_reopen_and_preserves_context_history() {
    let (url, path) = test_db();
    let store = Store::connect(&url).await.unwrap();

    let agent = store
        .register_agent("codex-test", &["code".into()])
        .await
        .unwrap();
    let task = store
        .create_task(CreateTask {
            project_id: None,
            title: "Implement vertical slice".into(),
            description: "test".into(),
            owner_actor_id: "human:test".into(),
            state: TaskState::Ready,
            priority: 10,
        })
        .await
        .unwrap();

    let c1 = store
        .create_context_revision(
            task.id,
            CreateContextRevision {
                goal: "first goal".into(),
                background: "background".into(),
                constraints: json!({"no_push": true}),
                current_summary: "start".into(),
                created_by_actor_id: "human:test".into(),
            },
        )
        .await
        .unwrap();
    let c2 = store
        .create_context_revision(
            task.id,
            CreateContextRevision {
                goal: "updated goal".into(),
                background: "background".into(),
                constraints: json!({"no_push": true}),
                current_summary: "more context".into(),
                created_by_actor_id: "human:test".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(c1.version, 1);
    assert_eq!(c2.version, 2);
    assert_eq!(c2.parent_revision_id, Some(c1.id));

    let assignment = store
        .claim_task(task.id, agent.id, "executor", 300)
        .await
        .unwrap();
    let run = store
        .start_run(assignment.id, agent.id, Some("external-session-1".into()))
        .await
        .unwrap();
    store
        .checkpoint_run(
            run.id,
            agent.id,
            json!({"commit":"abc123","remaining":["tests"]}),
        )
        .await
        .unwrap();

    drop(store);
    let reopened = Store::connect(&url).await.unwrap();
    let recovered = reopened.get_run(run.id).await.unwrap();
    assert_eq!(recovered.checkpoint.unwrap()["commit"], "abc123");

    reopened
        .complete_run(run.id, agent.id, json!({"ok":true}))
        .await
        .unwrap();
    assert_eq!(
        reopened.get_task(task.id).await.unwrap().state,
        TaskState::Done
    );

    let events = reopened.task_events(task.id).await.unwrap();
    let event_types: Vec<_> = events.iter().map(|e| e.event_type.as_str()).collect();
    assert!(event_types.contains(&"task.created"));
    assert!(event_types.contains(&"context.revised"));
    assert!(event_types.contains(&"assignment.claimed"));
    assert!(event_types.contains(&"run.started"));
    assert!(event_types.contains(&"run.checkpointed"));
    assert!(event_types.contains(&"run.completed"));

    drop(reopened);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn concurrent_claim_allows_only_one_active_executor() {
    let (url, path) = test_db();
    let store = Store::connect(&url).await.unwrap();
    let a = store.register_agent("agent-a", &[]).await.unwrap();
    let b = store.register_agent("agent-b", &[]).await.unwrap();
    let task = store
        .create_task(CreateTask {
            project_id: None,
            title: "Race".into(),
            description: String::new(),
            owner_actor_id: "human:test".into(),
            state: TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap();

    let sa = store.clone();
    let sb = store.clone();
    let (ra, rb) = tokio::join!(
        sa.claim_task(task.id, a.id, "executor", 300),
        sb.claim_task(task.id, b.id, "executor", 300)
    );

    assert_ne!(ra.is_ok(), rb.is_ok(), "exactly one claim must succeed");
    let loser = if ra.is_err() {
        ra.unwrap_err()
    } else {
        rb.unwrap_err()
    };
    assert!(matches!(loser, DomainError::Conflict(_)));

    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn run_mutations_enforce_assignment_owner() {
    let (url, path) = test_db();
    let store = Store::connect(&url).await.unwrap();
    let owner = store.register_agent("owner", &[]).await.unwrap();
    let intruder = store.register_agent("intruder", &[]).await.unwrap();
    let task = store
        .create_task(CreateTask {
            project_id: None,
            title: "Ownership".into(),
            description: String::new(),
            owner_actor_id: "human:test".into(),
            state: TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap();
    let assignment = store
        .claim_task(task.id, owner.id, "executor", 300)
        .await
        .unwrap();

    assert!(matches!(
        store
            .start_run(assignment.id, intruder.id, None)
            .await
            .unwrap_err(),
        DomainError::Conflict(_)
    ));

    let run = store
        .start_run(assignment.id, owner.id, None)
        .await
        .unwrap();
    assert!(matches!(
        store
            .checkpoint_run(run.id, intruder.id, json!({"bad":true}))
            .await
            .unwrap_err(),
        DomainError::Conflict(_)
    ));

    drop(store);
    let _ = std::fs::remove_file(path);
}
