use morrows_core::*;
use morrows_store::Store;
use serde_json::json;
use uuid::Uuid;

async fn task(store: &Store, title: &str) -> Task {
    store
        .create_task(CreateTask {
            project_id: None,
            title: title.into(),
            description: String::new(),
            owner_actor_id: "human:test".into(),
            state: TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap()
}
fn handoff() -> CreateHandoff {
    CreateHandoff {
        summary: "Parser implemented; validate next".into(),
        completed: vec!["parser".into()],
        remaining: vec!["tests".into()],
        blockers: vec![],
        artifact_ids: vec![],
        decision_ids: vec![],
    }
}
async fn context(store: &Store, id: Id, summary: &str) -> ContextRevision {
    store
        .create_context_revision(
            id,
            CreateContextRevision {
                goal: "Ship parser".into(),
                background: "No shared chat required".into(),
                constraints: json!({"no_push":true}),
                current_summary: summary.into(),
                created_by_actor_id: "human:test".into(),
            },
        )
        .await
        .unwrap()
}
#[tokio::test]
async fn handoff_continuation_survives_restart_with_all_collaboration_records() {
    let path = std::env::temp_dir().join(format!("ac-m2-{}.db", Uuid::new_v4()));
    let url = format!("sqlite://{}", path.display());
    let store = Store::connect(&url).await.unwrap();
    let a = store.register_agent("a", &[]).await.unwrap();
    let b = store.register_agent("b", &[]).await.unwrap();
    let t = task(&store, "Continue").await;
    let original = context(&store, t.id, "original").await;
    let assignment = store.claim_task(t.id, a.id, "executor", 300).await.unwrap();
    let run = store
        .start_run(assignment.id, a.id, Some("private-a-chat".into()))
        .await
        .unwrap();
    assert!(store.start_run(assignment.id, a.id, None).await.is_err());
    store
        .checkpoint_run(run.id, a.id, json!({"completed":["parser"]}))
        .await
        .unwrap();
    let artifact = store
        .create_artifact(
            t.id,
            a.id,
            CreateArtifact {
                kind: "other".into(),
                title: "Parser patch".into(),
                uri: "file:///work/parser.rs".into(),
                description: "Ready for tests".into(),
            },
        )
        .await
        .unwrap();
    let decision = store
        .create_decision(
            t.id,
            a.id,
            CreateDecision {
                title: "Use recursive descent".into(),
                rationale: "Grammar is small".into(),
            },
        )
        .await
        .unwrap();
    let thread = store
        .create_thread(
            t.id,
            a.id,
            CreateThread {
                title: "Review notes".into(),
            },
        )
        .await
        .unwrap();
    store
        .create_message(
            thread.id,
            a.id,
            serde_json::from_value(json!({"body":"Check empty input"})).unwrap(),
        )
        .await
        .unwrap();
    let mut input = handoff();
    input.artifact_ids.push(artifact.id.to_string());
    input.decision_ids.push(decision.id.to_string());
    assert!(matches!(
        store.create_handoff(run.id, b.id, input.clone()).await,
        Err(DomainError::Conflict(_))
    ));
    let h = store.create_handoff(run.id, a.id, input).await.unwrap();
    assert_eq!(
        store.get_assignment(assignment.id).await.unwrap().status,
        "released"
    );
    assert_ne!(store.get_task(t.id).await.unwrap().state, TaskState::Done);
    for id in [run.id] {
        assert_eq!(store.get_run(id).await.unwrap().status, "handed_off");
        assert!(store.complete_run(id, a.id, json!({})).await.is_err());
        assert!(store.checkpoint_run(id, a.id, json!({})).await.is_err());
    }
    assert!(store.create_handoff(run.id, a.id, handoff()).await.is_err());
    context(&store, t.id, "later revision").await;
    drop(store);
    let store = Store::connect(&url).await.unwrap();
    let recovered = store.get_handoff(h.id).await.unwrap();
    assert_eq!(recovered.context.id, original.id);
    assert_eq!(recovered.context.constraints["no_push"], true);
    assert_eq!(recovered.handoff.content.remaining, vec!["tests"]);
    assert_eq!(recovered.artifacts[0].id, artifact.id);
    assert_eq!(recovered.decisions[0].id, decision.id);
    let all = store.task_collaboration(t.id).await.unwrap();
    assert_eq!(all.messages[0].body, "Check empty input");
    assert_eq!(all.threads[0].id, thread.id);
    let next = store.claim_task(t.id, b.id, "executor", 300).await.unwrap();
    let run_b = store
        .start_run(next.id, b.id, Some("independent-b-chat".into()))
        .await
        .unwrap();
    store
        .complete_run(
            run_b.id,
            b.id,
            json!({"tests":"passed","continued_from":h.id}),
        )
        .await
        .unwrap();
    assert_eq!(store.get_task(t.id).await.unwrap().state, TaskState::Done);
    let events = store.task_events(t.id).await.unwrap();
    for kind in [
        "handoff.created",
        "artifact.created",
        "decision.created",
        "thread.created",
        "message.created",
    ] {
        assert_eq!(events.iter().filter(|e| e.event_type == kind).count(), 1);
    }
    drop(store);
    let _ = std::fs::remove_file(path);
}
#[tokio::test]
async fn failed_handoff_rolls_back_and_rejects_cross_task_references() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = store.register_agent("a", &[]).await.unwrap();
    let t = task(&store, "source").await;
    let other = task(&store, "other").await;
    let assignment = store.claim_task(t.id, a.id, "executor", 300).await.unwrap();
    let run = store.start_run(assignment.id, a.id, None).await.unwrap();
    assert!(matches!(
        store.create_handoff(run.id, a.id, handoff()).await,
        Err(DomainError::InvalidInput(_))
    ));
    context(&store, t.id, "context").await;
    let artifact = store
        .create_artifact(
            other.id,
            a.id,
            CreateArtifact {
                kind: "other".into(),
                title: "other".into(),
                uri: "file:///other".into(),
                description: String::new(),
            },
        )
        .await
        .unwrap();
    let mut invalid = handoff();
    invalid.artifact_ids.push(artifact.id.to_string());
    assert!(matches!(
        store.create_handoff(run.id, a.id, invalid).await,
        Err(DomainError::InvalidInput(_))
    ));
    assert_eq!(store.get_run(run.id).await.unwrap().status, "running");
    assert_eq!(
        store.get_assignment(assignment.id).await.unwrap().status,
        "active"
    );
    assert!(store.task_handoffs(t.id).await.unwrap().is_empty());
    assert!(
        !store
            .task_events(t.id)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event_type == "handoff.created")
    );
    assert!(
        store
            .create_message(
                Uuid::new_v4(),
                a.id,
                serde_json::from_value(json!({"body":"missing thread"})).unwrap()
            )
            .await
            .is_err()
    );
    assert!(
        store
            .create_decision(
                t.id,
                a.id,
                CreateDecision {
                    title: " ".into(),
                    rationale: "why".into()
                }
            )
            .await
            .is_err()
    );
}
#[tokio::test]
async fn dependencies_reject_cycles_and_gate_executor_until_prerequisite_completes() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = store.register_agent("a", &[]).await.unwrap();
    let t = task(&store, "dependent").await;
    let prerequisite = task(&store, "prerequisite").await;
    let third = task(&store, "third").await;
    store
        .add_dependency(t.id, prerequisite.id, a.id)
        .await
        .unwrap();
    store.add_dependency(third.id, t.id, a.id).await.unwrap();
    assert!(
        store
            .add_dependency(prerequisite.id, third.id, a.id)
            .await
            .is_err()
    );
    assert!(store.add_dependency(t.id, t.id, a.id).await.is_err());
    assert!(
        store
            .add_dependency(t.id, prerequisite.id, a.id)
            .await
            .is_err()
    );
    assert!(matches!(
        store.claim_task(t.id, a.id, "executor", 300).await,
        Err(DomainError::Conflict(_))
    ));
    let review = store.claim_task(t.id, a.id, "reviewer", 300).await.unwrap();
    let run = store.start_run(review.id, a.id, None).await.unwrap();
    store.complete_run(run.id, a.id, json!({})).await.unwrap();
    assert_ne!(store.get_task(t.id).await.unwrap().state, TaskState::Done);
    let assignment = store
        .claim_task(prerequisite.id, a.id, "executor", 300)
        .await
        .unwrap();
    let run = store.start_run(assignment.id, a.id, None).await.unwrap();
    store.complete_run(run.id, a.id, json!({})).await.unwrap();
    store.claim_task(t.id, a.id, "executor", 300).await.unwrap();
    store.remove_dependency(third.id, t.id, a.id).await.unwrap();
    assert!(store.task_dependencies(third.id).await.unwrap().is_empty());
    let events = store.task_events(third.id).await.unwrap();
    assert!(events.iter().any(|e| e.event_type == "dependency.removed"));
}
#[tokio::test]
async fn concurrent_handoffs_and_reverse_dependencies_have_one_winner() {
    let path = std::env::temp_dir().join(format!("ac-race-{}.db", Uuid::new_v4()));
    let store = Store::connect(&format!("sqlite://{}", path.display()))
        .await
        .unwrap();
    let a = store.register_agent("a", &[]).await.unwrap();
    let t = task(&store, "a").await;
    let other = task(&store, "b").await;
    let (x, y) = tokio::join!(
        store.add_dependency(t.id, other.id, a.id),
        store.add_dependency(other.id, t.id, a.id)
    );
    assert_ne!(x.is_ok(), y.is_ok());
    if x.is_ok() {
        store.remove_dependency(t.id, other.id, a.id).await.unwrap();
    }
    context(&store, t.id, "context").await;
    let assignment = store.claim_task(t.id, a.id, "executor", 300).await.unwrap();
    let run = store.start_run(assignment.id, a.id, None).await.unwrap();
    let (x, y) = tokio::join!(
        store.create_handoff(run.id, a.id, handoff()),
        store.create_handoff(run.id, a.id, handoff())
    );
    assert_ne!(x.is_ok(), y.is_ok());
    assert_eq!(store.task_handoffs(t.id).await.unwrap().len(), 1);
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn dependency_added_during_run_blocks_completion_without_partial_changes() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = store.register_agent("a", &[]).await.unwrap();
    let t = task(&store, "running").await;
    let prerequisite = task(&store, "prerequisite").await;
    let assignment = store.claim_task(t.id, a.id, "executor", 300).await.unwrap();
    let run = store.start_run(assignment.id, a.id, None).await.unwrap();
    store
        .add_dependency(t.id, prerequisite.id, a.id)
        .await
        .unwrap();
    assert!(matches!(
        store.complete_run(run.id, a.id, json!({})).await,
        Err(DomainError::Conflict(_))
    ));
    assert_eq!(store.get_run(run.id).await.unwrap().status, "running");
    assert_eq!(
        store.get_assignment(assignment.id).await.unwrap().status,
        "active"
    );
    assert!(
        !store
            .task_events(t.id)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event_type == "run.completed")
    );
    store
        .remove_dependency(t.id, prerequisite.id, a.id)
        .await
        .unwrap();
    store.complete_run(run.id, a.id, json!({})).await.unwrap();
}

#[tokio::test]
async fn expired_assignment_cannot_handoff_or_complete_after_reassignment() {
    let path = std::env::temp_dir().join(format!("ac-expiry-{}.db", Uuid::new_v4()));
    let url = format!("sqlite://{}", path.display());
    let store = Store::connect(&url).await.unwrap();
    let a = store.register_agent("a", &[]).await.unwrap();
    let b = store.register_agent("b", &[]).await.unwrap();
    let t = task(&store, "expired").await;
    context(&store, t.id, "context").await;
    let assignment = store.claim_task(t.id, a.id, "executor", 300).await.unwrap();
    let run = store.start_run(assignment.id, a.id, None).await.unwrap();
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    sqlx::query("UPDATE assignments SET expires_at='2000-01-01T00:00:00Z' WHERE id=?")
        .bind(assignment.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.create_handoff(run.id, a.id, handoff()).await.is_err());
    assert!(store.complete_run(run.id, a.id, json!({})).await.is_err());
    assert_eq!(store.expire_stale_assignments().await.unwrap(), 1);
    let next = store.claim_task(t.id, b.id, "executor", 300).await.unwrap();
    assert!(store.complete_run(run.id, a.id, json!({})).await.is_err());
    assert_eq!(
        store.get_assignment(next.id).await.unwrap().status,
        "active"
    );
    pool.close().await;
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn directed_messages_and_replies_persist_and_validate_relationships() {
    let path = std::env::temp_dir().join(format!("ac-messages-{}.db", Uuid::new_v4()));
    let url = format!("sqlite://{}", path.display());
    let store = Store::connect(&url).await.unwrap();
    let a = store.register_agent("a", &[]).await.unwrap();
    let b = store.register_agent("b", &[]).await.unwrap();
    let t = task(&store, "messages").await;
    let thread = store
        .create_thread(
            t.id,
            a.id,
            CreateThread {
                title: "review".into(),
            },
        )
        .await
        .unwrap();
    let input = json!({"body":"Please review","message_type":"request",
        "recipient_agent_instance_id":b.id,"recipient_role":"reviewer",
        "correlation_id":"review-1","requires_response":true,"status":"sent"});
    let message = store
        .create_message(
            thread.id,
            a.id,
            serde_json::from_value(input.clone()).unwrap(),
        )
        .await
        .unwrap();
    let reply = store
        .create_message(
            thread.id,
            b.id,
            serde_json::from_value(json!({
                "body":"Reviewed","message_type":"reply","recipient_agent_instance_id":a.id,
                "reply_to_message_id":message.id,"correlation_id":"review-1"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let other_task = task(&store, "other").await;
    let other = store
        .create_thread(
            other_task.id,
            a.id,
            CreateThread {
                title: "other".into(),
            },
        )
        .await
        .unwrap();
    for (thread_id, fields) in [
        (
            other.id,
            json!({"body":"invalid","reply_to_message_id":message.id}),
        ),
        (
            thread.id,
            json!({"body":"invalid","reply_to_message_id":Uuid::new_v4()}),
        ),
        (
            thread.id,
            json!({"body":"invalid","recipient_agent_instance_id":Uuid::new_v4()}),
        ),
        (
            thread.id,
            json!({"body":"invalid","recipient_agent_instance_id":"bad-id"}),
        ),
        (thread.id, json!({"body":"invalid","recipient_role":" "})),
    ] {
        assert!(
            store
                .create_message(thread_id, a.id, serde_json::from_value(fields).unwrap())
                .await
                .is_err()
        );
    }
    let artifact = store
        .create_artifact(
            t.id,
            a.id,
            serde_json::from_value(json!({
                "title":"patch","uri":"file:///patch","kind":"patch"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let legacy = store
        .create_artifact(
            t.id,
            a.id,
            serde_json::from_value(json!({
                "title":"legacy","uri":"file:///legacy"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(legacy.content.kind, "other");
    drop(store);
    let store = Store::connect(&url).await.unwrap();
    let messages = store.thread_messages(thread.id).await.unwrap();
    assert_eq!(messages.len(), 2);
    let saved = serde_json::to_value(&messages[0]).unwrap();
    for (key, value) in input.as_object().unwrap() {
        assert_eq!(&saved[key], value);
    }
    assert_eq!(messages[1].id, reply.id);
    assert_eq!(
        messages[1].reply_to_message_id,
        Some(message.id.to_string())
    );
    assert_eq!(
        messages[1].recipient_agent_instance_id,
        Some(a.id.to_string())
    );
    assert_eq!(messages[1].correlation_id.as_deref(), Some("review-1"));
    assert!(!messages[1].requires_response);
    assert_eq!(messages[1].status, "sent");
    assert_eq!(
        store
            .task_artifacts(t.id)
            .await
            .unwrap()
            .iter()
            .find(|x| x.id == artifact.id)
            .unwrap()
            .content
            .kind,
        "patch"
    );
    assert_eq!(
        store
            .task_events(t.id)
            .await
            .unwrap()
            .iter()
            .filter(|e| e.event_type == "message.created")
            .count(),
        2
    );
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn handoff_acceptance_validates_target_and_is_atomic_and_durable() {
    let path = std::env::temp_dir().join(format!("ac-accept-{}.db", Uuid::new_v4()));
    let url = format!("sqlite://{}", path.display());
    let store = Store::connect(&url).await.unwrap();
    let a = store.register_agent("source", &[]).await.unwrap();
    let b = store.register_agent("target", &[]).await.unwrap();
    let t = task(&store, "handoff").await;
    context(&store, t.id, "continue").await;
    let assignment = store.claim_task(t.id, a.id, "executor", 300).await.unwrap();
    let source = store.start_run(assignment.id, a.id, None).await.unwrap();
    let h = store
        .create_handoff(source.id, a.id, handoff())
        .await
        .unwrap();
    assert_eq!(h.status, "pending");
    assert_eq!(h.accepted_by_run_id, None);
    let next = store.claim_task(t.id, b.id, "executor", 300).await.unwrap();
    let target = store.start_run(next.id, b.id, None).await.unwrap();
    assert!(store.start_run(next.id, b.id, None).await.is_err());
    let unrelated = task(&store, "unrelated").await;
    let wrong_assignment = store
        .claim_task(unrelated.id, a.id, "executor", 300)
        .await
        .unwrap();
    let wrong_run = store
        .start_run(wrong_assignment.id, a.id, None)
        .await
        .unwrap();
    // Neither ownership alone nor a same-task run belonging to someone else suffices.
    for (run, actor) in [(wrong_run.id, a.id), (target.id, a.id), (source.id, a.id)] {
        assert!(matches!(
            store.accept_handoff(h.id, run, actor).await,
            Err(DomainError::Conflict(_))
        ));
    }
    let completed_assignment = store.claim_task(t.id, b.id, "reviewer", 300).await.unwrap();
    let completed = store
        .start_run(completed_assignment.id, b.id, None)
        .await
        .unwrap();
    store
        .complete_run(completed.id, b.id, json!({}))
        .await
        .unwrap();
    assert!(
        store
            .accept_handoff(h.id, completed.id, b.id)
            .await
            .is_err()
    );
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    sqlx::query("UPDATE assignments SET expires_at='2000-01-01T00:00:00Z' WHERE id=?")
        .bind(next.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert!(store.accept_handoff(h.id, target.id, b.id).await.is_err());
    sqlx::query("UPDATE assignments SET expires_at='2999-01-01T00:00:00Z' WHERE id=?")
        .bind(next.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store.get_handoff(h.id).await.unwrap().handoff.status,
        "pending"
    );
    assert!(
        !store
            .task_events(t.id)
            .await
            .unwrap()
            .iter()
            .any(|e| e.event_type == "handoff.accepted")
    );
    let (first, second) = tokio::join!(
        store.accept_handoff(h.id, target.id, b.id),
        store.accept_handoff(h.id, target.id, b.id)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let accepted = first.or(second).unwrap();
    assert_eq!(accepted.status, "accepted");
    assert!(store.accept_handoff(h.id, target.id, b.id).await.is_err());
    pool.close().await;
    drop(store);
    let store = Store::connect(&url).await.unwrap();
    let saved = store.get_handoff(h.id).await.unwrap().handoff;
    assert_eq!(saved.status, "accepted");
    assert_eq!(saved.accepted_by_run_id, accepted.accepted_by_run_id);
    let events = store.task_events(t.id).await.unwrap();
    let events: Vec<_> = events
        .iter()
        .filter(|e| e.event_type == "handoff.accepted")
        .collect();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].payload["accepted_by_run_id"],
        json!(saved.accepted_by_run_id)
    );
    assert_eq!(store.get_run(target.id).await.unwrap().status, "running");
    drop(store);
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn m21_migration_preserves_existing_m2_records() {
    use sqlx::Row;
    let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::raw_sql(include_str!("../migrations/0001_init.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!("../migrations/0002_collaboration.sql"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::raw_sql("
        INSERT INTO agent_instances(id,name,last_heartbeat_at) VALUES('a','legacy','2026-01-01');
        INSERT INTO tasks(id,title,description,owner_actor_id,state,created_at,updated_at)
            VALUES('t','legacy','','human:test','ready','2026-01-01','2026-01-01');
        INSERT INTO assignments(id,task_id,role,agent_instance_id,status,acquired_at,expires_at,renewed_at)
            VALUES('as','t','executor','a','released','2026-01-01','2026-01-02','2026-01-01');
        INSERT INTO runs(id,task_id,assignment_id,agent_instance_id,status,started_at)
            VALUES('r','t','as','a','handed_off','2026-01-01');
        INSERT INTO context_revisions(id,task_id,version,created_by_actor_id,created_at)
            VALUES('c','t',1,'human:test','2026-01-01');
        INSERT INTO artifacts VALUES('artifact','t','a','patch','file:///patch','legacy description','2026-01-01');
        INSERT INTO message_threads VALUES('thread','t','a','notes','2026-01-01');
        INSERT INTO messages VALUES('message','thread','a','legacy body','2026-01-01');
        INSERT INTO handoffs VALUES('h','t','r','a','c','{}','2026-01-01');
    ").execute(&pool).await.unwrap();
    sqlx::raw_sql(include_str!(
        "../migrations/0003_messaging_handoff_acceptance.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();
    let artifact = sqlx::query("SELECT * FROM artifacts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(artifact.get::<String, _>("kind"), "other");
    assert_eq!(
        artifact.get::<String, _>("description"),
        "legacy description"
    );
    let message = sqlx::query("SELECT * FROM messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(message.get::<String, _>("body"), "legacy body");
    assert_eq!(message.get::<String, _>("message_type"), "note");
    assert_eq!(message.get::<String, _>("status"), "sent");
    assert!(!message.get::<bool, _>("requires_response"));
    for field in [
        "recipient_agent_instance_id",
        "recipient_role",
        "reply_to_message_id",
        "correlation_id",
    ] {
        assert_eq!(message.get::<Option<String>, _>(field), None);
    }
    let handoff = sqlx::query("SELECT * FROM handoffs")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(handoff.get::<String, _>("status"), "pending");
    assert_eq!(handoff.get::<Option<String>, _>("accepted_by_run_id"), None);
    // Storage also prevents partially accepted handoffs, even outside the store API.
    assert!(
        sqlx::query("UPDATE handoffs SET status='accepted'")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("UPDATE handoffs SET accepted_by_run_id='r'")
            .execute(&pool)
            .await
            .is_err()
    );
}
