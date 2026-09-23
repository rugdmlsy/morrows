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
        .set_run_capability(run_id, Some("cap_1"))
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
