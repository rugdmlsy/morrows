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
async fn conversation_message_enters_delivery_outbox_and_only_target_can_ack() {
    let (store, a, b, _, _, _) = setup().await;
    let conversation = store
        .create_conversation(CreateConversation {
            agent_instance_id: a.id,
            title: "Direct".into(),
        })
        .await
        .unwrap();
    let message = store
        .create_human_conversation_message(conversation.id, "hello delivery")
        .await
        .unwrap();

    let inbox = store.agent_delivery_inbox(a.id, 80).await.unwrap();
    assert_eq!(inbox.len(), 1);
    let delivery = &inbox[0];
    assert_eq!(delivery.kind, "conversation_message");
    assert_eq!(delivery.source_id, message.id);
    assert_eq!(
        delivery.payload["conversation_id"],
        conversation.id.to_string()
    );
    assert_eq!(delivery.payload["body"], "hello delivery");

    let summary = store
        .list_conversations(Some(a.id))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(summary.queued_count, 1);
    assert_eq!(summary.undelivered_count, 1);
    let history = store
        .conversation_history(conversation.id, None, None, 20)
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

    let summary = store
        .list_conversations(Some(a.id))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(summary.queued_count, 1, "delivery is not a reply");
    assert_eq!(summary.undelivered_count, 0);
    let history = store
        .conversation_history(conversation.id, None, None, 20)
        .await
        .unwrap();
    assert_eq!(
        history.messages[0].delivery_status.as_deref(),
        Some("delivered")
    );
}

#[tokio::test]
async fn conversation_reply_atomically_consumes_pending_delivery() {
    let (store, a, _, _, _, _) = setup().await;
    let conversation = store
        .create_conversation(CreateConversation {
            agent_instance_id: a.id,
            title: "Reply".into(),
        })
        .await
        .unwrap();
    store
        .create_human_conversation_message(conversation.id, "please answer")
        .await
        .unwrap();
    assert_eq!(store.agent_delivery_inbox(a.id, 80).await.unwrap().len(), 1);

    store
        .agent_reply_conversation(conversation.id, a.id, "answered")
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
