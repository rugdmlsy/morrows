use morrows_core::*;
use morrows_store::Store;
use serde_json::{Value, json};

fn input<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

async fn setup() -> (
    Store,
    AgentInstance,
    AgentInstance,
    Task,
    Assignment,
    LaunchProfile,
) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = store.register_agent("delivery-a", &[]).await.unwrap();
    let b = store.register_agent("delivery-b", &[]).await.unwrap();
    let task = store
        .create_task(input(json!({"title":"delivery task"})))
        .await
        .unwrap();
    let assignment = store
        .claim_task(task.id, a.id, "executor", 600)
        .await
        .unwrap();
    let profile = store
        .register_launch_profile(input(json!({
            "name":"delivery codex",
            "adapter":"codex_cli",
            "agent_instance_id":a.id,
            "program":"/bin/echo",
            "default_cwd":"/tmp"
        })))
        .await
        .unwrap();
    (store, a, b, task, assignment, profile)
}

#[tokio::test]
async fn session_message_enters_delivery_outbox_and_only_target_can_ack() {
    let (store, a, b, _, _, _) = setup().await;
    let session = store
        .create_session(CreateSession {
            agent_instance_id: a.id,
            title: "Direct".into(),
        })
        .await
        .unwrap();
    let message = store
        .create_human_session_message(session.id, "hello delivery")
        .await
        .unwrap();

    let inbox = store.agent_delivery_inbox(a.id, 80).await.unwrap();
    assert_eq!(inbox.len(), 1);
    let delivery = &inbox[0];
    assert_eq!(delivery.kind, "session_message");
    assert_eq!(delivery.source_id, message.id);
    assert_eq!(delivery.payload["session_id"], session.id.to_string());
    assert_eq!(delivery.payload["body"], "hello delivery");

    let summary = store.list_sessions(Some(a.id)).await.unwrap().remove(0);
    assert_eq!(summary.queued_count, 1);
    assert_eq!(summary.undelivered_count, 1);
    let history = store
        .session_history(session.id, None, None, 20)
        .await
        .unwrap();
    assert_eq!(
        history.messages[0].delivery_status.as_deref(),
        Some("queued")
    );

    assert!(matches!(
        store
            .acknowledge_agent_delivery(delivery.id, b.id, "test")
            .await,
        Err(DomainError::Conflict(_))
    ));

    let delivered = store
        .acknowledge_agent_delivery(delivery.id, a.id, "test-bridge")
        .await
        .unwrap();
    assert_eq!(delivered.status, "delivered");
    assert_eq!(delivered.delivered_by.as_deref(), Some("test-bridge"));
    assert!(
        store
            .agent_delivery_inbox(a.id, 80)
            .await
            .unwrap()
            .is_empty()
    );

    let summary = store.list_sessions(Some(a.id)).await.unwrap().remove(0);
    assert_eq!(summary.queued_count, 1, "delivery is not a reply");
    assert_eq!(summary.undelivered_count, 0);
    let history = store
        .session_history(session.id, None, None, 20)
        .await
        .unwrap();
    assert_eq!(
        history.messages[0].delivery_status.as_deref(),
        Some("delivered")
    );
}

#[tokio::test]
async fn session_reply_atomically_consumes_pending_delivery() {
    let (store, a, _, _, _, _) = setup().await;
    let session = store
        .create_session(CreateSession {
            agent_instance_id: a.id,
            title: "Reply".into(),
        })
        .await
        .unwrap();
    store
        .create_human_session_message(session.id, "please answer")
        .await
        .unwrap();
    assert_eq!(store.agent_delivery_inbox(a.id, 80).await.unwrap().len(), 1);

    store
        .agent_reply_session(session.id, a.id, "answered")
        .await
        .unwrap();

    assert!(
        store
            .agent_delivery_inbox(a.id, 80)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn launch_instruction_enters_task_scoped_delivery_and_claim_is_recoverable() {
    let (store, a, _, task, assignment, profile) = setup().await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment.id,
            "launch_profile_id":profile.id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
    store
        .mark_launch_running(
            attempt.id,
            Some(123),
            "/tmp/stdout".into(),
            "/tmp/stderr".into(),
        )
        .await
        .unwrap();

    let instruction = store
        .send_launch_instruction(
            SendLaunchInstruction {
                launch_attempt_id: attempt.id,
                body: "Inspect the new evidence".into(),
            },
            None,
        )
        .await
        .unwrap();
    let queued = store.agent_delivery_inbox(a.id, 80).await.unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].kind, "launch_instruction");
    assert_eq!(queued[0].source_id, instruction.id);
    assert_eq!(queued[0].task_id, Some(task.id));

    let claimed = store
        .claim_agent_deliveries_for_launch(attempt.id, 50)
        .await
        .unwrap();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].status, "claimed");
    assert_eq!(
        claimed[0].claimed_by_launch_attempt_id,
        Some(execution.attempt.id)
    );
    assert!(
        store
            .agent_delivery_inbox(a.id, 80)
            .await
            .unwrap()
            .is_empty()
    );

    assert_eq!(
        store
            .release_claimed_agent_deliveries(attempt.id)
            .await
            .unwrap(),
        1
    );
    assert_eq!(store.agent_delivery_inbox(a.id, 80).await.unwrap().len(), 1);

    let claimed_again = store
        .claim_agent_deliveries_for_launch(attempt.id, 50)
        .await
        .unwrap();
    assert_eq!(claimed_again.len(), 1);
    assert_eq!(
        store
            .complete_claimed_agent_deliveries(attempt.id, "codex_cli:test")
            .await
            .unwrap(),
        1
    );
    let delivered = store.get_agent_delivery(claimed_again[0].id).await.unwrap();
    assert_eq!(delivered.status, "delivered");
    assert_eq!(delivered.delivered_by.as_deref(), Some("codex_cli:test"));
}

#[tokio::test]
async fn abandoned_local_claim_is_returned_to_queue() {
    let (store, a, _, _, assignment, profile) = setup().await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment.id,
            "launch_profile_id":profile.id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    store.begin_launch_attempt(attempt.id).await.unwrap();
    store
        .mark_launch_running(
            attempt.id,
            Some(123),
            "/tmp/stdout".into(),
            "/tmp/stderr".into(),
        )
        .await
        .unwrap();
    store
        .send_launch_instruction(
            SendLaunchInstruction {
                launch_attempt_id: attempt.id,
                body: "Recover me".into(),
            },
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .claim_agent_deliveries_for_launch(attempt.id, 50)
            .await
            .unwrap()
            .len(),
        1
    );

    store
        .finish_launch_attempt(attempt.id, Some(1), None, Some("crash".into()))
        .await
        .unwrap();
    assert_eq!(store.recover_agent_delivery_claims().await.unwrap(), 1);
    assert_eq!(store.agent_delivery_inbox(a.id, 80).await.unwrap().len(), 1);
}

#[tokio::test]
async fn session_message_client_id_is_idempotent_and_queued_message_can_be_recalled() {
    let (store, a, _, _, _, _) = setup().await;
    let session = store
        .create_session(CreateSession {
            agent_instance_id: a.id,
            title: "Recall".into(),
        })
        .await
        .unwrap();

    let first = store
        .create_human_session_message_idempotent(session.id, "send once", Some("client-message-1"))
        .await
        .unwrap();
    let duplicate = store
        .create_human_session_message_idempotent(session.id, "send once", Some("client-message-1"))
        .await
        .unwrap();

    assert_eq!(first.id, duplicate.id);
    assert_eq!(first.client_message_id.as_deref(), Some("client-message-1"));
    assert_eq!(store.agent_delivery_inbox(a.id, 80).await.unwrap().len(), 1);
    assert!(matches!(
        store
            .create_human_session_message_idempotent(
                session.id,
                "different body",
                Some("client-message-1"),
            )
            .await,
        Err(DomainError::Conflict(_))
    ));

    let recalled = store
        .recall_human_session_message(session.id, first.id)
        .await
        .unwrap();
    assert!(recalled.recalled_at.is_some());
    assert!(recalled.delivery_status.is_none());
    assert!(
        store
            .agent_delivery_inbox(a.id, 80)
            .await
            .unwrap()
            .is_empty()
    );

    let summary = store.list_sessions(Some(a.id)).await.unwrap().remove(0);
    assert_eq!(summary.message_count, 0);
    assert_eq!(summary.queued_count, 0);
    assert_eq!(summary.undelivered_count, 0);
    assert!(summary.last_message_preview.is_none());

    let recalled_again = store
        .recall_human_session_message(session.id, first.id)
        .await
        .unwrap();
    assert_eq!(recalled_again.id, first.id);
    assert!(recalled_again.recalled_at.is_some());
}

#[tokio::test]
async fn session_message_cannot_be_recalled_after_delivery() {
    let (store, a, _, _, _, _) = setup().await;
    let session = store
        .create_session(CreateSession {
            agent_instance_id: a.id,
            title: "Delivered".into(),
        })
        .await
        .unwrap();
    let message = store
        .create_human_session_message(session.id, "already delivered")
        .await
        .unwrap();
    let delivery = store
        .agent_delivery_inbox(a.id, 80)
        .await
        .unwrap()
        .remove(0);
    store
        .acknowledge_agent_delivery(delivery.id, a.id, "test-runtime")
        .await
        .unwrap();

    assert!(matches!(
        store
            .recall_human_session_message(session.id, message.id)
            .await,
        Err(DomainError::Conflict(_))
    ));
}

#[tokio::test]
async fn session_message_cannot_be_recalled_after_runtime_claim() {
    let (store, a, _, _, assignment, profile) = setup().await;
    let session = store
        .create_scoped_session(
            CreateSession {
                agent_instance_id: a.id,
                title: "Claimed".into(),
            },
            None,
            Some(assignment.task_id),
        )
        .await
        .unwrap();
    let message = store
        .create_human_session_message(session.id, "claimed delivery")
        .await
        .unwrap();

    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment.id,
            "launch_profile_id": profile.id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    store.begin_launch_attempt(attempt.id).await.unwrap();
    store
        .mark_launch_running(
            attempt.id,
            Some(123),
            "/tmp/stdout".into(),
            "/tmp/stderr".into(),
        )
        .await
        .unwrap();
    let claimed = store
        .claim_agent_deliveries_for_launch(attempt.id, 50)
        .await
        .unwrap();
    assert!(
        claimed
            .iter()
            .any(|delivery| delivery.source_id == message.id)
    );

    assert!(matches!(
        store
            .recall_human_session_message(session.id, message.id)
            .await,
        Err(DomainError::Conflict(_))
    ));
}
