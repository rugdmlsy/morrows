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
    store.bind_run_runtime(run_id, "s_first").await.unwrap();
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
        .run_runtime_binding(run_id)
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
            .run_runtime_binding(run_id)
            .await
            .unwrap()
            .unwrap()
            .runtime_scope_id,
        "s_first"
    );
    assert_eq!(store.get_run(run_id).await.unwrap().status, "running");
    assert!(
        store
            .run_runtime_binding(run_id)
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
        .bind_run_runtime(execution.run.id, "s_cancel")
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
    let final_run = store
        .complete_runtime_cleanup(execution.run.id)
        .await
        .unwrap();
    assert_eq!(final_run.status, "cancelled");
    let events = store
        .task_events(store.get_assignment(assignment_id).await.unwrap().task_id)
        .await
        .unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.event_type == "runtime.scope_terminalized")
            .count(),
        1,
        "cancelled Run cleanup must report actual scope terminalization"
    );
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
        .bind_run_runtime(next_execution.run.id, "s_new")
        .await
        .unwrap();
    assert_eq!(
        store
            .run_runtime_binding(next_execution.run.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_scope_id,
        "s_new"
    );
}

#[tokio::test]
async fn cancelling_active_task_preserves_lsm_cleanup_and_terminal_task_state() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment_id,"launch_profile_id":profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
    store
        .bind_run_runtime(execution.run.id, "s_task_cancel")
        .await
        .unwrap();

    let task = store.cancel_task(assignment.task_id).await.unwrap();
    assert_eq!(task.state, TaskState::Cancelled);
    assert_eq!(
        store.get_run(execution.run.id).await.unwrap().status,
        "cancelling"
    );
    assert_eq!(
        store.get_assignment(assignment_id).await.unwrap().status,
        "released"
    );
    assert_eq!(
        store.get_launch_attempt(attempt.id).await.unwrap().status,
        "cancelled"
    );
    assert!(store.launch_stop_requested(attempt.id).await.unwrap());

    let final_run = store
        .complete_runtime_cleanup(execution.run.id)
        .await
        .unwrap();
    assert_eq!(final_run.status, "cancelled");
    assert_eq!(
        store.get_task(assignment.task_id).await.unwrap().state,
        TaskState::Cancelled,
        "runtime cleanup must not reopen a task that was explicitly cancelled"
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
async fn active_unbound_runtime_binding_is_replayable_after_daemon_restart() {
    let (store, assignment_id, profile_id) = prepared().await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment_id, "launch_profile_id": profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store
        .begin_launch_attempt_with_runtime(attempt.id, Some("shared-runtime"))
        .await
        .unwrap();
    let run_id = execution.run.id;
    assert_eq!(
        store
            .run_runtime_provisioning_subject(run_id)
            .await
            .unwrap()
            .as_deref(),
        Some("shared-runtime")
    );
    assert!(store.run_runtime_binding(run_id).await.unwrap().is_none());

    // The morrow-runtime scope POST may already have succeeded when Morrows dies.
    // Managed processes are not terminalized like local children. The same Run stays
    // active, its durable provisioning key is discoverable, and replay binds the
    // recovered scope to that Run without creating a second work execution.
    assert_eq!(store.recover_launch_jobs_after_restart().await.unwrap(), 0);
    assert_eq!(store.get_run(run_id).await.unwrap().status, "running");
    assert_eq!(
        store.get_assignment(assignment_id).await.unwrap().status,
        "active"
    );
    assert_eq!(store.unbound_runtime_runs().await.unwrap(), vec![run_id]);
    let recovered = store
        .bind_run_runtime(run_id, "scope_replayed")
        .await
        .unwrap();
    assert_eq!(recovered.run_id, run_id);
    assert_eq!(recovered.runtime_scope_id, "scope_replayed");
    assert!(recovered.restart_deadline_at.is_none());
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
        .begin_launch_attempt_with_local_compat(attempt.id, Some("shared-runtime"))
        .await
        .unwrap();
    let run_id = execution.run.id;
    assert_eq!(
        store.request_run_cancel(run_id).await.unwrap().status,
        "cancelling"
    );
    assert_eq!(
        store.pending_runtime_cleanup_runs().await.unwrap(),
        vec![run_id]
    );
    store
        .finish_launch_attempt(attempt.id, None, None, Some("cancel requested".into()))
        .await
        .unwrap();
    store
        .bind_run_runtime(run_id, "s_recovered_for_cleanup")
        .await
        .unwrap();
    assert_eq!(
        store.complete_runtime_cleanup(run_id).await.unwrap().status,
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
        .begin_launch_attempt_with_local_compat(attempt.id, Some("shared-runtime"))
        .await
        .unwrap()
        .run
        .id;
    store
        .bind_run_runtime(run_id, "s_issue_race")
        .await
        .unwrap();

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
            .run_runtime_binding(run_id)
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
        .begin_launch_attempt_with_local_compat(attempt.id, Some("shared-runtime"))
        .await
        .unwrap()
        .run
        .id;
    store
        .bind_run_runtime(run_id, "s_rotation_race")
        .await
        .unwrap();
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
            .run_runtime_binding(run_id)
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
        .begin_launch_attempt_with_local_compat(first.id, Some("shared-runtime"))
        .await
        .unwrap();
    store
        .bind_run_runtime(execution.run.id, "s_same_run")
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
    assert_eq!(
        restart_two.resume_from_attempt_id,
        Some(restart_one.id),
        "restart follows the latest LaunchAttempt while provider continuity remains owned by the Run"
    );
    assert_eq!(
        store
            .get_run(execution.run.id)
            .await
            .unwrap()
            .provider_conversation_ref
            .as_deref(),
        Some("codex-conversation")
    );
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
    store.bind_run_runtime(run_id, "s_delivery").await.unwrap();
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
            .run_runtime_binding(run_id)
            .await
            .unwrap()
            .unwrap()
            .runtime_scope_id,
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
    store.bind_run_runtime(run_id, "s_delivery").await.unwrap();
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
    assert!(store.delivery_resume_candidates().await.unwrap().is_empty());

    let unrelated_task = store
        .create_task(input(json!({"title":"unrelated delivery task"})))
        .await
        .unwrap();
    store
        .create_human_task_message(
            unrelated_task.id,
            assignment.agent_instance_id,
            "do not wake task run",
            None,
        )
        .await
        .unwrap();
    assert!(
        store.delivery_resume_candidates().await.unwrap().is_empty(),
        "a message for another Task must not wake an interrupted Task Run"
    );

    store
        .create_human_task_message(
            assignment.task_id,
            assignment.agent_instance_id,
            "continue this turn",
            None,
        )
        .await
        .unwrap();

    assert_eq!(
        store.delivery_resume_candidates().await.unwrap(),
        vec![run_id]
    );
    let resumed = store.enqueue_run_delivery_resume(run_id).await.unwrap();
    assert_eq!(resumed.restart_run_id, Some(run_id));
    assert_eq!(resumed.resume_from_attempt_id, Some(first.id));
    assert!(store.delivery_resume_candidates().await.unwrap().is_empty());
}

#[tokio::test]
async fn handoff_waits_for_child_exit_and_lsm_terminalization() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
    store.create_context_revision(assignment.task_id,input(json!({"goal":"handoff","background":"test","constraints":{},"current_summary":"work remains","created_by_actor_id":"human:test"}))).await.unwrap();
    let attempt = store
        .enqueue_launch(input(
            json!({"assignment_id":assignment_id,"launch_profile_id":profile_id}),
        ))
        .await
        .unwrap();
    assert!(!store.launch_stop_requested(attempt.id).await.unwrap());
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
    store
        .bind_run_runtime(execution.run.id, "s_handoff")
        .await
        .unwrap();
    let milestone = store
        .create_run_milestone(
            execution.run.id,
            assignment.agent_instance_id,
            input(json!({
                "kind":"budget_pressure",
                "summary":"quota exhausted; preserve uncommitted work location",
                "completed":[],
                "verified":["runtime is still attached to s_handoff"],
                "remaining":["finish"],
                "blockers":["provider quota exhausted"],
                "next_step":"resume from uncommitted.rs after predecessor runtime is terminal",
                "next_plan":["wait for predecessor runtime terminalization","re-open uncommitted.rs","verify live environment before continuing"],
                "execution_locations":["uncommitted.rs","lsm:s_handoff"],
                "artifact_ids":[],
                "decision_ids":[]
            })),
        )
        .await
        .unwrap();
    store.create_handoff(execution.run.id,assignment.agent_instance_id,input(json!({"milestone_id":milestone.id,"summary":"quota exhausted","remaining":["finish"],"completed":[],"blockers":[],"artifact_ids":[],"decision_ids":[]}))).await.unwrap();
    assert!(store.launch_stop_requested(attempt.id).await.unwrap());
    let successor = store.register_agent("successor", &[]).await.unwrap();
    assert!(
        store
            .claim_task(assignment.task_id, successor.id, "executor", 600)
            .await
            .is_err()
    );
    store
        .finish_launch_attempt(attempt.id, None, None, Some("run_cancel_requested".into()))
        .await
        .unwrap();
    assert_eq!(
        store.get_run(execution.run.id).await.unwrap().status,
        "handed_off"
    );
    assert!(
        store
            .completed_runtime_runs()
            .await
            .unwrap()
            .contains(&execution.run.id)
    );
    assert!(
        store
            .claim_task(assignment.task_id, successor.id, "executor", 600)
            .await
            .is_err()
    );
    store
        .mark_runtime_scope_terminalized(execution.run.id)
        .await
        .unwrap();
    let next = store
        .claim_task(assignment.task_id, successor.id, "executor", 600)
        .await
        .unwrap();
    assert_eq!(next.task_id, assignment.task_id);
    let recovered = store
        .task_recovery_context(next.task_id, successor.id)
        .await
        .unwrap();
    assert_eq!(recovered["milestone"]["id"], json!(milestone.id));
    assert_eq!(recovered["milestone"]["kind"], "budget_pressure");
    assert_eq!(
        recovered["checkpoint"]["execution_locations"],
        json!(["uncommitted.rs", "lsm:s_handoff"])
    );
    assert_eq!(
        recovered["checkpoint"]["next_step"],
        "resume from uncommitted.rs after predecessor runtime is terminal"
    );
}

#[tokio::test]
async fn abrupt_failure_retains_checkpoint_for_successor() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
    let attempt = store
        .enqueue_launch(input(
            json!({"assignment_id":assignment_id,"launch_profile_id":profile_id}),
        ))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
    store
        .checkpoint_run(
            execution.run.id,
            assignment.agent_instance_id,
            json!({"last_durable":"before quota failure"}),
        )
        .await
        .unwrap();
    store
        .finish_launch_attempt(
            attempt.id,
            Some(1),
            None,
            Some("account usage limit".into()),
        )
        .await
        .unwrap();
    let successor = store.register_agent("recovery", &[]).await.unwrap();
    let next = store
        .claim_task(assignment.task_id, successor.id, "executor", 600)
        .await
        .unwrap();
    let recovery = store
        .task_execution_page(next.task_id, successor.id, 20, 0)
        .await
        .unwrap();
    assert_eq!(
        recovery["recovery"]["source_run_id"],
        json!(execution.run.id)
    );
    assert_eq!(
        recovery["recovery"]["checkpoint"]["last_durable"],
        "before quota failure"
    );
    assert_eq!(recovery["recovery"]["stop_reason"], "account usage limit");
    assert!(
        store
            .checkpoint_run(execution.run.id, assignment.agent_instance_id, json!({}))
            .await
            .is_err()
    );
    assert!(
        store
            .complete_run(execution.run.id, assignment.agent_instance_id, json!({}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn lsm_job_wait_terminal_event_resumes_same_run_without_new_run() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
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
    store.bind_run_runtime(run_id, "s_job_wait").await.unwrap();

    let wait = store
        .register_run_job_wait(
            RegisterRunJobWait {
                run_id,
                source_machine: "morrow-node-01".into(),
                job_id: "job-long".into(),
                resume_plan: "Read the experiment report and continue the same task.".into(),
                reason: "Long experiment is still running.".into(),
            },
            assignment.agent_instance_id,
        )
        .await
        .unwrap();
    assert_eq!(wait.status, "pending");

    store
        .finish_launch_attempt(first.id, Some(0), Some("codex-wait-session".into()), None)
        .await
        .unwrap();
    assert_eq!(store.get_run(run_id).await.unwrap().status, "interrupted");
    assert_eq!(
        store
            .run_runtime_binding(run_id)
            .await
            .unwrap()
            .unwrap()
            .restart_deadline_at,
        None,
        "an intentional long wait must not start the short crash-restart timer"
    );

    let event = LsmJobTerminalEvent {
        event_id: "job-finish:job-long:1".into(),
        job_id: "job-long".into(),
        source_machine: "morrow-node-01".into(),
        logical_session_id: Some("s_job_wait".into()),
        attempt: 1,
        status: "succeeded".into(),
        exit_code: Some(0),
        completed_at: chrono::Utc::now(),
        terminal_reason: "process exited successfully".into(),
        summary_ref: Some("work/report.json".into()),
        result: Some(json!({"rows": 147})),
    };
    assert!(
        store
            .ingest_lsm_job_terminal_event(event.clone())
            .await
            .unwrap()
    );
    assert!(
        !store.ingest_lsm_job_terminal_event(event).await.unwrap(),
        "duplicate terminal events must be idempotent"
    );

    let ready = store.get_run_job_wait(wait.id).await.unwrap();
    assert_eq!(ready.status, "ready");
    assert_eq!(ready.resume_mode.as_deref(), Some("continue"));
    assert_eq!(ready.terminal_status.as_deref(), Some("succeeded"));
    assert_eq!(ready.summary_ref.as_deref(), Some("work/report.json"));

    let candidates = store.run_job_wait_resume_candidates().await.unwrap();
    assert_eq!(candidates.len(), 1);
    let resumed = store
        .enqueue_run_job_wait_resume(candidates[0])
        .await
        .unwrap();
    assert_eq!(resumed.restart_run_id, Some(run_id));
    assert_eq!(resumed.resume_from_attempt_id, Some(first.id));
    assert!(
        store
            .run_job_wait_resume_candidates()
            .await
            .unwrap()
            .is_empty()
    );

    let binding = store.run_runtime_binding(run_id).await.unwrap().unwrap();
    assert!(
        binding.restart_deadline_at.is_some(),
        "the short restart window starts only after the awaited event arrives"
    );
    assert!(
        store
            .get_assignment(assignment_id)
            .await
            .unwrap()
            .expires_at
            > chrono::Utc::now(),
        "wake-up must refresh the Assignment before relaunch"
    );

    store.claim_launch_job().await.unwrap().unwrap();
    let next = store.begin_launch_attempt(resumed.id).await.unwrap();
    assert_eq!(next.run.id, run_id);
    assert_eq!(store.get_run(run_id).await.unwrap().status, "running");
    assert_eq!(
        store
            .run_runtime_binding(run_id)
            .await
            .unwrap()
            .unwrap()
            .runtime_scope_id,
        "s_job_wait"
    );
    let events = store.task_events(assignment.task_id).await.unwrap();
    for name in ["run.job_wait_registered", "run.job_wait_ready"] {
        let found: Vec<_> = events
            .iter()
            .filter(|event| event.event_type == name)
            .collect();
        assert_eq!(
            found.len(),
            1,
            "{name} must appear exactly once in Task events"
        );
        assert_eq!(found[0].payload["task_id"], json!(assignment.task_id));
    }
}

#[tokio::test]
async fn lsm_terminal_event_before_wait_registration_is_not_lost() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
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
    store
        .bind_run_runtime(run_id, "s_event_first")
        .await
        .unwrap();

    assert!(
        store
            .ingest_lsm_job_terminal_event(LsmJobTerminalEvent {
                event_id: "job-finish:event-first:1".into(),
                job_id: "job-event-first".into(),
                source_machine: "morrow-node-01".into(),
                logical_session_id: Some("s_event_first".into()),
                attempt: 1,
                status: "failed".into(),
                exit_code: Some(2),
                completed_at: chrono::Utc::now(),
                terminal_reason: "process exited with code 2".into(),
                summary_ref: None,
                result: None,
            })
            .await
            .unwrap()
    );

    let wait = store
        .register_run_job_wait(
            RegisterRunJobWait {
                run_id,
                source_machine: "morrow-node-01".into(),
                job_id: "job-event-first".into(),
                resume_plan: "Inspect the failed job and decide the next action.".into(),
                reason: "The external job may have finished while the Agent was exiting.".into(),
            },
            assignment.agent_instance_id,
        )
        .await
        .unwrap();
    assert_eq!(wait.status, "ready");
    assert_eq!(wait.terminal_status.as_deref(), Some("failed"));

    store
        .finish_launch_attempt(first.id, Some(0), Some("event-first-session".into()), None)
        .await
        .unwrap();
    assert_eq!(
        store.run_job_wait_resume_candidates().await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn lost_lsm_job_wakes_same_run_in_reconciliation_mode() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
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
    store.bind_run_runtime(run_id, "s_lost_wait").await.unwrap();

    let wait = store
        .register_run_job_wait(
            RegisterRunJobWait {
                run_id,
                source_machine: "morrow-node-01".into(),
                job_id: "job-lost".into(),
                resume_plan: "Reconcile durable outputs before deciding whether to retry.".into(),
                reason: "Wait for a long-running job.".into(),
            },
            assignment.agent_instance_id,
        )
        .await
        .unwrap();
    store
        .finish_launch_attempt(first.id, Some(0), Some("lost-session".into()), None)
        .await
        .unwrap();

    store
        .ingest_lsm_job_terminal_event(LsmJobTerminalEvent {
            event_id: "job-finish:lost:1".into(),
            job_id: "job-lost".into(),
            source_machine: "morrow-node-01".into(),
            logical_session_id: Some("s_lost_wait".into()),
            attempt: 1,
            status: "lost".into(),
            exit_code: None,
            completed_at: chrono::Utc::now(),
            terminal_reason: "job session disappeared without a durable completion record".into(),
            summary_ref: None,
            result: None,
        })
        .await
        .unwrap();

    let ready = store.get_run_job_wait(wait.id).await.unwrap();
    assert_eq!(ready.status, "ready");
    assert_eq!(ready.resume_mode.as_deref(), Some("reconcile"));
    assert_eq!(ready.terminal_status.as_deref(), Some("lost"));
    assert_eq!(
        store.run_job_wait_resume_candidates().await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn unrelated_lsm_terminal_events_are_accepted_but_not_persisted_in_morrows() {
    let (store, _assignment_id, _profile_id) = prepared().await;
    let inserted = store
        .ingest_lsm_job_terminal_event(LsmJobTerminalEvent {
            event_id: "job-finish:unrelated:1".into(),
            job_id: "job-unrelated".into(),
            source_machine: "some-node".into(),
            logical_session_id: Some("s_not_a_morrows_run".into()),
            attempt: 1,
            status: "succeeded".into(),
            exit_code: Some(0),
            completed_at: chrono::Utc::now(),
            terminal_reason: "process exited successfully".into(),
            summary_ref: None,
            result: None,
        })
        .await
        .unwrap();
    assert!(!inserted);
}

#[tokio::test]
async fn runtime_access_is_bound_to_the_run_executor() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment_id,
            "launch_profile_id": profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();

    let (run, task) = store
        .authorize_run_runtime_access(execution.run.id, assignment.agent_instance_id)
        .await
        .unwrap();
    assert_eq!(run.id, execution.run.id);
    assert_eq!(task.id, run.task_id);

    let other = store
        .register_agent("other-runtime-agent".into(), &[])
        .await
        .unwrap();
    let denied = store
        .authorize_run_runtime_access(execution.run.id, other.id)
        .await
        .unwrap_err();
    assert!(denied.to_string().contains("not owned"));
}

#[tokio::test]
async fn adhoc_runtime_binding_is_stable_per_agent_and_machine() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent_a = store.register_agent("adhoc-a".into(), &[]).await.unwrap();
    let agent_b = store.register_agent("adhoc-b".into(), &[]).await.unwrap();
    let machine_a = store
        .register_machine(input(json!({
            "name":"adhoc-mac","hostname":"mac.local","status":"online"
        })))
        .await
        .unwrap();
    let machine_b = store
        .register_machine(input(json!({
            "name":"adhoc-node","hostname":"node.local","status":"online"
        })))
        .await
        .unwrap();

    let initial = store
        .ensure_adhoc_runtime_binding(agent_a.id, machine_a.id)
        .await
        .unwrap();
    assert_eq!(initial.generation, 1);
    assert!(initial.runtime_scope_id.is_none());

    let bound = store
        .bind_adhoc_runtime_scope(agent_a.id, machine_a.id, 1, "s_adhoc_a_mac_g1")
        .await
        .unwrap();
    assert_eq!(bound.runtime_scope_id.as_deref(), Some("s_adhoc_a_mac_g1"));

    let replay = store
        .ensure_adhoc_runtime_binding(agent_a.id, machine_a.id)
        .await
        .unwrap();
    assert_eq!(replay.runtime_scope_id, bound.runtime_scope_id);
    assert_eq!(replay.generation, 1);

    let other_machine = store
        .ensure_adhoc_runtime_binding(agent_a.id, machine_b.id)
        .await
        .unwrap();
    let other_agent = store
        .ensure_adhoc_runtime_binding(agent_b.id, machine_a.id)
        .await
        .unwrap();
    assert_ne!(other_machine.machine_id, bound.machine_id);
    assert_ne!(other_agent.agent_instance_id, bound.agent_instance_id);
    assert!(other_machine.runtime_scope_id.is_none());
    assert!(other_agent.runtime_scope_id.is_none());

    let rotated = store
        .rotate_adhoc_runtime_binding(agent_a.id, machine_a.id, 1)
        .await
        .unwrap();
    assert_eq!(rotated.generation, 2);
    assert!(rotated.runtime_scope_id.is_none());

    let rebound = store
        .bind_adhoc_runtime_scope(agent_a.id, machine_a.id, 2, "s_adhoc_a_mac_g2")
        .await
        .unwrap();
    assert_eq!(rebound.generation, 2);
    assert_eq!(
        rebound.runtime_scope_id.as_deref(),
        Some("s_adhoc_a_mac_g2")
    );
    assert!(
        store
            .bind_adhoc_runtime_scope(agent_a.id, machine_a.id, 1, "stale")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn active_implementing_run_blocks_adhoc_mode() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
    assert!(
        !store
            .has_active_implementing_executor_run(assignment.agent_instance_id)
            .await
            .unwrap()
    );
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment_id,
            "launch_profile_id":profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
    assert_eq!(execution.run.status, "running");
    assert!(
        store
            .has_active_implementing_executor_run(assignment.agent_instance_id)
            .await
            .unwrap()
    );
    let other = store
        .register_agent("adhoc-other".into(), &[])
        .await
        .unwrap();
    assert!(
        !store
            .has_active_implementing_executor_run(other.id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn runtime_scope_observability_records_first_bind_and_verified_cleanup_once() {
    let (store, assignment_id, profile_id) = prepared().await;
    let assignment = store.get_assignment(assignment_id).await.unwrap();
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id": assignment_id, "launch_profile_id":profile_id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let run = store.begin_launch_attempt(attempt.id).await.unwrap().run;

    store
        .bind_run_runtime(run.id, "scope_test_only")
        .await
        .unwrap();
    store
        .bind_run_runtime(run.id, "scope_test_only")
        .await
        .unwrap();
    let prior = store.task_events(assignment.task_id).await.unwrap();
    let bound: Vec<_> = prior
        .iter()
        .filter(|event| event.event_type == "runtime.scope_bound")
        .collect();
    assert_eq!(
        bound.len(),
        1,
        "idempotent rebind must not duplicate events"
    );
    assert_eq!(bound[0].entity_id, run.id);
    assert_eq!(
        bound[0].payload,
        json!({"task_id":run.task_id}),
        "scope identifiers and capability tokens must not be emitted"
    );

    store.mark_runtime_scope_terminalized(run.id).await.unwrap();
    store.mark_runtime_scope_terminalized(run.id).await.unwrap();
    let after = store.task_events(assignment.task_id).await.unwrap();
    let terminal: Vec<_> = after
        .iter()
        .filter(|event| event.event_type == "runtime.scope_terminalized")
        .collect();
    assert_eq!(
        terminal.len(),
        1,
        "verified terminalization must be recorded exactly once"
    );
    assert_eq!(terminal[0].payload, json!({"task_id":run.task_id}));
    assert_eq!(terminal[0].actor_type, "system");
    assert_eq!(
        store
            .run_runtime_binding(run.id)
            .await
            .unwrap()
            .unwrap()
            .runtime_scope_id,
        "scope_test_only",
        "event instrumentation may not destroy the durable scope"
    );
}
