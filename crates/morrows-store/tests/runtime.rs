use morrows_core::*;
use morrows_store::Store;
use serde_json::{Value, json};

fn input<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

async fn prepared() -> (Store, Id, Id) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store
        .register_agent("runtime-agent".into(), &[])
        .await
        .unwrap();
    let task = store
        .create_task(input(json!({"title":"runtime task"})))
        .await
        .unwrap();
    let assignment = store
        .claim_task(task.id, agent.id, "executor", 600)
        .await
        .unwrap();
    let profile = store
        .register_launch_profile(input(json!({
            "name":"runtime codex", "adapter":"codex_cli", "agent_instance_id":agent.id,
            "program":"/bin/echo", "default_cwd":"/tmp"
        })))
        .await
        .unwrap();
    (store, assignment.id, profile.id)
}

#[tokio::test]
async fn interrupted_run_restarts_with_one_primary_session() {
    let (store, assignment_id, profile_id) = prepared().await;
    let first = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment_id,"launch_profile_id":profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let first_execution = store.begin_launch_attempt(first.id).await.unwrap();
    let run_id = first_execution.run.id;
    store.bind_run_lsm(run_id, "s_first").await.unwrap();
    store
        .try_adopt_run_capability(run_id, "cap_1")
        .await
        .unwrap();
    store
        .finish_launch_attempt(first.id, Some(1), None, Some("agent died".into()))
        .await
        .unwrap();
    assert_eq!(store.get_run(run_id).await.unwrap().status, "interrupted");
    assert_eq!(
        store.get_assignment(assignment_id).await.unwrap().status,
        "active"
    );
    let deadline = store
        .run_lsm_binding(run_id)
        .await
        .unwrap()
        .unwrap()
        .restart_deadline_at
        .unwrap();
    let remaining = (deadline - chrono::Utc::now()).num_seconds();
    assert!((598..=600).contains(&remaining));

    let restart = store.enqueue_run_restart(run_id).await.unwrap();
    assert_eq!(restart.restart_run_id, Some(run_id));
    store.claim_launch_job().await.unwrap().unwrap();
    let second_execution = store.begin_launch_attempt(restart.id).await.unwrap();
    assert_eq!(second_execution.run.id, run_id);
    assert_eq!(
        store
            .run_lsm_binding(run_id)
            .await
            .unwrap()
            .unwrap()
            .logical_session_id,
        "s_first"
    );
    assert_eq!(store.get_run(run_id).await.unwrap().status, "running");
    assert!(
        store
            .run_lsm_binding(run_id)
            .await
            .unwrap()
            .unwrap()
            .restart_deadline_at
            .is_none()
    );
}

#[tokio::test]
async fn cancelling_is_not_cancelled_until_cleanup_confirms() {
    let (store, assignment_id, profile_id) = prepared().await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment_id,"launch_profile_id":profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
    store
        .bind_run_lsm(execution.run.id, "s_cancel")
        .await
        .unwrap();
    store.request_run_cancel(execution.run.id).await.unwrap();
    assert_eq!(
        store.get_run(execution.run.id).await.unwrap().status,
        "cancelling"
    );
    assert!(store.launch_stop_requested(attempt.id).await.unwrap());
    store
        .finish_launch_attempt(attempt.id, None, None, Some("run_cancel_requested".into()))
        .await
        .unwrap();
    assert_eq!(
        store.get_run(execution.run.id).await.unwrap().status,
        "cancelling"
    );
    let final_run = store.complete_lsm_cleanup(execution.run.id).await.unwrap();
    assert_eq!(final_run.status, "cancelled");
    assert_eq!(final_run.failure_reason, None);
    let old_assignment = store.get_assignment(assignment_id).await.unwrap();
    let next_assignment = store
        .claim_task(
            old_assignment.task_id,
            old_assignment.agent_instance_id,
            "executor",
            600,
        )
        .await
        .unwrap();
    let next_attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":next_assignment.id,"launch_profile_id":profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let next_execution = store.begin_launch_attempt(next_attempt.id).await.unwrap();
    assert_ne!(next_execution.run.id, execution.run.id);
    store
        .bind_run_lsm(next_execution.run.id, "s_new")
        .await
        .unwrap();
    assert_eq!(
        store
            .run_lsm_binding(next_execution.run.id)
            .await
            .unwrap()
            .unwrap()
            .logical_session_id,
        "s_new"
    );
}

#[tokio::test]
async fn one_semantic_event_can_have_multiple_execution_evidence_refs() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment_id, "launch_profile_id": profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
    let started = store
        .task_events(assignment.task_id)
        .await
        .unwrap()
        .into_iter()
        .find(|event| event.event_type == "run.started" && event.entity_id == execution.run.id)
        .unwrap();
    for (kind, reference) in [("audit_range", "12:19"), ("job", "j_failed")] {
        store
            .attach_run_execution_evidence(
                execution.run.id,
                input(json!({
                    "event_id": started.id, "kind": kind, "reference": reference,
                    "start_seq": 12, "end_seq": 19
                })),
            )
            .await
            .unwrap();
    }
    let evidence = store
        .run_execution_evidence(execution.run.id)
        .await
        .unwrap();
    assert_eq!(evidence.len(), 2);
    assert!(
        evidence
            .iter()
            .all(|item| item.event_id == Some(started.id))
    );
    // A failed LSM Job is evidence for the Run, not a Run state transition.
    assert_eq!(
        store.get_run(execution.run.id).await.unwrap().status,
        "running"
    );
}

#[tokio::test]
async fn unbound_lsm_run_recovers_after_launcher_restart() {
    let (store, assignment_id, profile_id) = prepared().await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment_id, "launch_profile_id": profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store
        .begin_launch_attempt_with_lsm(attempt.id, Some("shared-runtime"))
        .await
        .unwrap();
    let run_id = execution.run.id;
    assert_eq!(
        store
            .run_lsm_provisioning_subject(run_id)
            .await
            .unwrap()
            .as_deref(),
        Some("shared-runtime")
    );
    assert!(store.run_lsm_binding(run_id).await.unwrap().is_none());

    // The LSM POST may already have succeeded when the Morrows process dies.
    // Startup recovery must retain the Run and its Assignment for keyed replay.
    assert_eq!(store.recover_launch_jobs_after_restart().await.unwrap(), 1);
    assert_eq!(store.get_run(run_id).await.unwrap().status, "interrupted");
    assert_eq!(
        store.get_assignment(assignment_id).await.unwrap().status,
        "active"
    );
    assert_eq!(store.unbound_lsm_runs().await.unwrap(), vec![run_id]);
    store.renew_interrupted_assignments().await.unwrap();
    let recovered = store.bind_run_lsm(run_id, "s_replayed").await.unwrap();
    assert!(recovered.restart_deadline_at.is_some());
    assert!(
        store
            .get_assignment(assignment_id)
            .await
            .unwrap()
            .expires_at
            >= recovered.restart_deadline_at.unwrap()
    );
    let restart = store.enqueue_run_restart(run_id).await.unwrap();
    assert_eq!(restart.restart_run_id, Some(run_id));
    assert_eq!(
        store
            .run_lsm_binding(run_id)
            .await
            .unwrap()
            .unwrap()
            .logical_session_id,
        "s_replayed"
    );
}

#[tokio::test]
async fn cancellation_intent_persists_before_an_unbound_session_is_recovered() {
    let (store, assignment_id, profile_id) = prepared().await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment_id, "launch_profile_id": profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store
        .begin_launch_attempt_with_lsm(attempt.id, Some("shared-runtime"))
        .await
        .unwrap();
    let run_id = execution.run.id;
    assert_eq!(
        store.request_run_cancel(run_id).await.unwrap().status,
        "cancelling"
    );
    assert_eq!(
        store.pending_lsm_cleanup_runs().await.unwrap(),
        vec![run_id]
    );
    store
        .finish_launch_attempt(attempt.id, None, None, Some("cancel requested".into()))
        .await
        .unwrap();
    store
        .bind_run_lsm(run_id, "s_recovered_for_cleanup")
        .await
        .unwrap();
    assert_eq!(
        store.complete_lsm_cleanup(run_id).await.unwrap().status,
        "cancelled"
    );
}

#[tokio::test]
async fn cancellation_wins_while_capability_issuance_is_paused() {
    let (store, assignment_id, profile_id) = prepared().await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment_id, "launch_profile_id": profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let run_id = store
        .begin_launch_attempt_with_lsm(attempt.id, Some("shared-runtime"))
        .await
        .unwrap()
        .run
        .id;
    store.bind_run_lsm(run_id, "s_issue_race").await.unwrap();

    let (issued_tx, issued_rx) = tokio::sync::oneshot::channel();
    let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
    let launcher_store = store.clone();
    let launcher = tokio::spawn(async move {
        issued_tx.send(()).unwrap();
        resume_rx.await.unwrap();
        launcher_store
            .try_adopt_run_capability(run_id, "new-capability")
            .await
    });
    issued_rx.await.unwrap();
    assert_eq!(
        store.request_run_cancel(run_id).await.unwrap().status,
        "cancelling"
    );
    resume_tx.send(()).unwrap();
    assert!(matches!(
        launcher.await.unwrap(),
        Err(DomainError::Conflict(_))
    ));
    assert!(
        store
            .run_lsm_binding(run_id)
            .await
            .unwrap()
            .unwrap()
            .capability_id
            .is_none()
    );
    assert!(matches!(
        store
            .mark_launch_running(attempt.id, Some(123), "stdout".into(), "stderr".into())
            .await,
        Err(DomainError::Conflict(_))
    ));
    assert_eq!(
        store.get_launch_attempt(attempt.id).await.unwrap().status,
        "starting"
    );
}

#[tokio::test]
async fn old_revoke_cannot_clear_a_newly_adopted_capability() {
    let (store, assignment_id, profile_id) = prepared().await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment_id, "launch_profile_id": profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let run_id = store
        .begin_launch_attempt_with_lsm(attempt.id, Some("shared-runtime"))
        .await
        .unwrap()
        .run
        .id;
    store.bind_run_lsm(run_id, "s_rotation_race").await.unwrap();
    store
        .try_adopt_run_capability(run_id, "old-capability")
        .await
        .unwrap();
    store
        .try_adopt_run_capability(run_id, "new-capability")
        .await
        .unwrap();
    store
        .clear_run_capability_if_matches(run_id, "old-capability")
        .await
        .unwrap();
    assert_eq!(
        store
            .run_lsm_binding(run_id)
            .await
            .unwrap()
            .unwrap()
            .capability_id
            .as_deref(),
        Some("new-capability")
    );
}

#[tokio::test]
async fn repeated_restart_uses_the_latest_known_codex_conversation() {
    let (store, assignment_id, profile_id) = prepared().await;
    let first = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment_id, "launch_profile_id": profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store
        .begin_launch_attempt_with_lsm(first.id, Some("shared-runtime"))
        .await
        .unwrap();
    store
        .bind_run_lsm(execution.run.id, "s_same_run")
        .await
        .unwrap();
    store
        .finish_launch_attempt(
            first.id,
            Some(7),
            Some("codex-conversation".into()),
            Some("first crash".into()),
        )
        .await
        .unwrap();

    let restart_one = store.enqueue_run_restart(execution.run.id).await.unwrap();
    assert_eq!(restart_one.resume_from_attempt_id, Some(first.id));
    store.claim_launch_job().await.unwrap().unwrap();
    store.begin_launch_attempt(restart_one.id).await.unwrap();
    store
        .finish_launch_attempt(restart_one.id, Some(7), None, Some("second crash".into()))
        .await
        .unwrap();

    let restart_two = store.enqueue_run_restart(execution.run.id).await.unwrap();
    assert_eq!(restart_two.resume_from_attempt_id, Some(first.id));
}

#[tokio::test]
async fn pending_delivery_can_resume_interrupted_codex_run_without_new_run() {
    let (store, assignment_id, profile_id) = prepared().await;
    let first = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment_id,
            "launch_profile_id": profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(first.id).await.unwrap();
    let run_id = execution.run.id;
    store.bind_run_lsm(run_id, "s_delivery").await.unwrap();
    store
        .mark_launch_running(
            first.id,
            Some(123),
            "/tmp/delivery-stdout".into(),
            "/tmp/delivery-stderr".into(),
        )
        .await
        .unwrap();

    store
        .send_launch_instruction(
            SendLaunchInstruction {
                launch_attempt_id: first.id,
                body: "Continue with the new evidence".into(),
            },
            None,
        )
        .await
        .unwrap();

    store
        .finish_launch_attempt(
            first.id,
            Some(0),
            Some("codex-delivery-session".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(store.get_run(run_id).await.unwrap().status, "interrupted");

    let candidates = store.delivery_resume_candidates().await.unwrap();
    assert_eq!(candidates, vec![run_id]);

    let resumed = store.enqueue_run_delivery_resume(run_id).await.unwrap();
    assert_eq!(resumed.restart_run_id, Some(run_id));
    assert_eq!(resumed.resume_from_attempt_id, Some(first.id));

    // A queued continuation fences a second delivery worker from enqueuing another.
    assert!(store.delivery_resume_candidates().await.unwrap().is_empty());

    store.claim_launch_job().await.unwrap().unwrap();
    let next = store.begin_launch_attempt(resumed.id).await.unwrap();
    assert_eq!(next.run.id, run_id);
    assert_eq!(store.get_run(run_id).await.unwrap().status, "running");
    assert_eq!(
        store
            .run_lsm_binding(run_id)
            .await
            .unwrap()
            .unwrap()
            .logical_session_id,
        "s_delivery"
    );
}

#[tokio::test]
async fn queued_delivery_makes_interrupted_codex_run_eligible_for_automatic_resume() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
    let first = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment_id,
            "launch_profile_id":profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(first.id).await.unwrap();
    let run_id = execution.run.id;
    store.bind_run_lsm(run_id, "s_delivery").await.unwrap();
    store
        .finish_launch_attempt(
            first.id,
            Some(0),
            Some("codex-delivery-session".into()),
            None,
        )
        .await
        .unwrap();
    assert_eq!(store.get_run(run_id).await.unwrap().status, "interrupted");
    assert!(
        store
            .delivery_resume_candidates()
            .await
            .unwrap()
            .is_empty()
    );

    let conversation = store
        .create_conversation(CreateConversation {
            agent_instance_id: assignment.agent_instance_id,
            title: "Delivery wake".into(),
        })
        .await
        .unwrap();
    store
        .create_human_conversation_message(conversation.id, "continue this turn")
        .await
        .unwrap();

    assert_eq!(
        store.delivery_resume_candidates().await.unwrap(),
        vec![run_id]
    );
    let resumed = store.enqueue_run_delivery_resume(run_id).await.unwrap();
    assert_eq!(resumed.restart_run_id, Some(run_id));
    assert_eq!(resumed.resume_from_attempt_id, Some(first.id));
    assert!(
        store
            .delivery_resume_candidates()
            .await
            .unwrap()
            .is_empty()
    );
}
