use morrows_core::*;
use morrows_store::Store;
use serde_json::{Value, json};

fn input<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

async fn fixture() -> (Store, Project, AgentInstance, Task) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(input(json!({
            "name":"intake project",
            "description":"durable project background"
        })))
        .await
        .unwrap();
    let agent = store.register_agent("intake-worker", &[]).await.unwrap();
    let task = store
        .create_task(input(json!({
            "project_id":project.id,
            "title":"gated work",
            "description":"implement only after intake"
        })))
        .await
        .unwrap();
    store
        .create_context_revision(
            task.id,
            input(json!({
                "goal":"ship safely",
                "background":"current task background",
                "constraints":{"acceptance_criteria":["intake enforced"]},
                "current_summary":"not started",
                "created_by_actor_id":"human:test"
            })),
        )
        .await
        .unwrap();
    (store, project, agent, task)
}

fn finalize(final_summary_message_id: Id, confirmation_message_id: Id) -> InterviewFinalize {
    InterviewFinalize {
        understanding: "Read current project/task context and incorporate the Human interview."
            .into(),
        constraints: json!({"preserve_existing_behavior":true}),
        plan: json!([
            "inspect current state",
            "implement only after conversational convergence",
            "verify acceptance criteria"
        ]),
        unresolved_questions: vec![],
        final_summary_message_id: Some(final_summary_message_id),
        confirmation_message_id: Some(confirmation_message_id),
    }
}

fn finalize_attested() -> InterviewFinalize {
    InterviewFinalize {
        understanding: "Read current project/task context and reconcile the implementation plan with the Human."
            .into(),
        constraints: json!({"preserve_existing_behavior":true}),
        plan: json!([
            "inspect current state",
            "implement the reconciled plan",
            "verify acceptance criteria"
        ]),
        unresolved_questions: vec![],
        final_summary_message_id: None,
        confirmation_message_id: None,
    }
}

#[tokio::test]
async fn executor_can_attest_convergence_without_persisted_interview_messages() {
    let (store, _project, agent, task) = fixture().await;
    let claim = store
        .claim_task_for_execution(task.id, agent.id, "executor", 300)
        .await
        .unwrap();
    store
        .task_intake_page(task.id, agent.id, 100, 0)
        .await
        .unwrap();
    let started = store
        .start_intake_interview(task.id, agent.id)
        .await
        .unwrap();
    let thread_id = started.interview_thread_id.unwrap();
    let history = store.thread_messages(thread_id).await.unwrap();
    assert!(history.is_empty());

    let converged = store
        .finalize_intake_interview(task.id, agent.id, finalize_attested())
        .await
        .unwrap();
    assert_eq!(converged.conversation_state, INTERVIEW_STATE_CONVERGED);
    assert_eq!(converged.final_summary_message_id, None);
    assert_eq!(converged.confirmation_message_id, None);
    assert_eq!(
        store
            .task_agent_thread(task.id, agent.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        thread_id
    );
    assert_eq!(
        store
            .get_assignment(claim.assignment.id)
            .await
            .unwrap()
            .phase,
        INTAKE_PHASE_READY
    );
    let implementing = store.begin_task_execution(task.id, agent.id).await.unwrap();
    assert_eq!(implementing.phase, INTAKE_PHASE_IMPLEMENTING);
    assert_eq!(
        store
            .task_agent_thread(task.id, agent.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        thread_id,
        "the Task collaboration transcript remains durable after implementation starts"
    );
}

#[tokio::test]
async fn open_claim_requires_multi_turn_human_interview_before_implementation() {
    let (store, _project, agent, task) = fixture().await;
    let claim = store
        .claim_task_for_execution(task.id, agent.id, "executor", 300)
        .await
        .unwrap();
    assert_eq!(claim.assignment.phase, INTAKE_PHASE_CONTEXT_REVIEW);
    let initial = store
        .get_assignment_intake(claim.assignment.id)
        .await
        .unwrap();
    assert_eq!(initial.conversation_state, INTERVIEW_STATE_NOT_STARTED);

    let view = store
        .task_intake_page(task.id, agent.id, 100, 0)
        .await
        .unwrap();
    assert!(view.intake.project_memory_complete);
    assert!(view.intake.context_package_id.is_some());
    assert!(
        view.blockers
            .iter()
            .any(|blocker| blocker == "human_interview_not_converged")
    );

    let started = store
        .start_intake_interview(task.id, agent.id)
        .await
        .unwrap();
    let thread_id = started.interview_thread_id.unwrap();
    assert_eq!(
        started.conversation_state,
        INTERVIEW_STATE_WAITING_FOR_AGENT
    );
    assert_eq!(
        store
            .get_assignment(claim.assignment.id)
            .await
            .unwrap()
            .phase,
        INTAKE_PHASE_HUMAN_INTERVIEW
    );

    store
        .agent_reply_task_thread(
            thread_id,
            agent.id,
            "I need one detail before implementation: which compatibility behavior must be preserved?",
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .get_assignment_intake(claim.assignment.id)
            .await
            .unwrap()
            .conversation_state,
        INTERVIEW_STATE_WAITING_FOR_HUMAN
    );

    store
        .create_human_task_message(
            task.id,
            agent.id,
            "Preserve existing operator recovery behavior.",
            None,
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .get_assignment_intake(claim.assignment.id)
            .await
            .unwrap()
            .conversation_state,
        INTERVIEW_STATE_WAITING_FOR_AGENT
    );

    store
        .agent_reply_task_thread(
            thread_id,
            agent.id,
            "Final synthesis: preserve operator recovery behavior; inspect, implement, then run the acceptance tests.",
        )
        .await
        .unwrap();

    let converged = store
        .finalize_intake_interview(task.id, agent.id, finalize_attested())
        .await
        .unwrap();
    assert_eq!(converged.conversation_state, INTERVIEW_STATE_CONVERGED);
    assert_eq!(converged.final_summary_message_id, None);
    assert_eq!(converged.confirmation_message_id, None);
    assert_eq!(
        converged.approved_by_actor_id,
        Some(format!("agent_attested:{}", agent.id))
    );
    assert_eq!(
        store
            .get_assignment(claim.assignment.id)
            .await
            .unwrap()
            .phase,
        INTAKE_PHASE_READY
    );

    // A new Human message before implementation reopens the interview instead of
    // letting an already-converged plan race into execution.
    store
        .create_human_task_message(task.id, agent.id, "再补充一点：保留旧日志格式。", None)
        .await
        .unwrap();
    let reopened = store
        .get_assignment_intake(claim.assignment.id)
        .await
        .unwrap();
    assert_eq!(
        reopened.conversation_state,
        INTERVIEW_STATE_WAITING_FOR_AGENT
    );
    assert!(reopened.converged_at.is_none());
    assert_eq!(
        store
            .get_assignment(claim.assignment.id)
            .await
            .unwrap()
            .phase,
        INTAKE_PHASE_HUMAN_INTERVIEW
    );

    store
        .agent_reply_task_thread(
            thread_id,
            agent.id,
            "Revised synthesis: preserve operator recovery and the existing log format; then implement and test.",
        )
        .await
        .unwrap();
    store
        .finalize_intake_interview(task.id, agent.id, finalize_attested())
        .await
        .unwrap();

    let implementing = store.begin_task_execution(task.id, agent.id).await.unwrap();
    assert_eq!(implementing.phase, INTAKE_PHASE_IMPLEMENTING);
}

#[tokio::test]
async fn context_change_invalidates_conversational_convergence() {
    let (store, _project, agent, task) = fixture().await;
    let claim = store
        .claim_task_for_execution(task.id, agent.id, "executor", 300)
        .await
        .unwrap();
    store
        .task_intake_page(task.id, agent.id, 100, 0)
        .await
        .unwrap();
    let intake = store
        .start_intake_interview(task.id, agent.id)
        .await
        .unwrap();
    let thread_id = intake.interview_thread_id.unwrap();
    let final_summary = store
        .agent_reply_task_thread(
            thread_id,
            agent.id,
            "Final plan: implement the current acceptance criteria.",
        )
        .await
        .unwrap();
    let confirmation = store
        .create_human_task_message(task.id, agent.id, "Proceed with that plan.", None)
        .await
        .unwrap();
    store
        .finalize_intake_interview(
            task.id,
            agent.id,
            finalize(final_summary.id, confirmation.id),
        )
        .await
        .unwrap();

    store
        .create_context_revision(
            task.id,
            input(json!({
                "goal":"ship safely",
                "background":"changed after conversational convergence",
                "constraints":{"acceptance_criteria":["intake enforced","new constraint"]},
                "current_summary":"context changed",
                "created_by_actor_id":"human:test"
            })),
        )
        .await
        .unwrap();

    let stale = store
        .begin_task_execution(task.id, agent.id)
        .await
        .unwrap_err();
    assert!(stale.to_string().contains("task_context_changed"));

    let refreshed = store
        .task_intake_page(task.id, agent.id, 100, 0)
        .await
        .unwrap();
    assert_eq!(refreshed.assignment.phase, INTAKE_PHASE_CONTEXT_REVIEW);
    assert_eq!(
        refreshed.intake.conversation_state,
        INTERVIEW_STATE_NOT_STARTED
    );
    assert!(refreshed.intake.converged_at.is_none());
    assert_eq!(
        refreshed.intake.interview_thread_id,
        Some(thread_id),
        "transcript remains durable even though convergence is invalidated"
    );
    assert_eq!(
        store
            .get_assignment(claim.assignment.id)
            .await
            .unwrap()
            .phase,
        INTAKE_PHASE_CONTEXT_REVIEW
    );
}

#[tokio::test]
async fn approved_assignment_cannot_start_run_before_intake() {
    let (store, _project, agent, task) = fixture().await;
    store
        .set_task_assignment_mode(task.id, AssignmentMode::Approval)
        .await
        .unwrap();
    let request = store
        .request_assignment(task.id, agent.id, "executor", "take this work")
        .await
        .unwrap();
    let resolved = store
        .resolve_assignment_request(request.id, None, "approve", "assigned", 300)
        .await
        .unwrap();
    let assignment_id = resolved.assignment_id.unwrap();
    let assignment = store.get_assignment(assignment_id).await.unwrap();
    assert_eq!(assignment.phase, INTAKE_PHASE_CONTEXT_REVIEW);

    let error = store
        .start_run(assignment_id, agent.id, None)
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("complete task_intake and human interview first")
    );
    assert!(store.task_runs(task.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn projectless_executor_intake_is_blocked_before_interview() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store
        .register_agent("unclassified-worker", &[])
        .await
        .unwrap();
    let task = store
        .create_task(input(json!({"title":"unclassified"})))
        .await
        .unwrap();
    store
        .claim_task_for_execution(task.id, agent.id, "executor", 300)
        .await
        .unwrap();

    let error = store
        .task_intake_page(task.id, agent.id, 20, 0)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("task has no project"));
}

#[tokio::test]
async fn human_messages_resume_read_only_intake_then_convergence_queues_implementation_launch() {
    let (store, _project, agent, task) = fixture().await;
    store
        .set_task_assignment_mode(task.id, AssignmentMode::Approval)
        .await
        .unwrap();
    let request = store
        .request_assignment(task.id, agent.id, "executor", "take this work")
        .await
        .unwrap();
    let resolved = store
        .resolve_assignment_request(request.id, None, "approve", "assigned", 300)
        .await
        .unwrap();
    let assignment_id = resolved.assignment_id.unwrap();
    store
        .task_intake_page(task.id, agent.id, 100, 0)
        .await
        .unwrap();
    let intake = store
        .start_intake_interview(task.id, agent.id)
        .await
        .unwrap();
    let thread_id = intake.interview_thread_id.unwrap();

    let program = std::env::current_exe().unwrap();
    let cwd = std::env::current_dir().unwrap();
    let profile = store
        .register_launch_profile(RegisterLaunchProfile {
            name: "intake-test-profile".into(),
            adapter: "codex_cli".into(),
            agent_instance_id: agent.id,
            program: program.to_string_lossy().into_owned(),
            default_cwd: Some(cwd.to_string_lossy().into_owned()),
            model: None,
            enabled: true,
            metadata: json!({}),
        })
        .await
        .unwrap();

    let first = store
        .enqueue_launch(EnqueueLaunch {
            assignment_id,
            launch_profile_id: profile.id,
            cwd: None,
            resume_from_attempt_id: None,
        })
        .await
        .unwrap();
    let first_execution = store
        .begin_launch_attempt_with_local_compat(first.id, Some("test-lsm-subject"))
        .await
        .unwrap();
    assert_eq!(
        store
            .run_runtime_provisioning_subject(first_execution.run.id)
            .await
            .unwrap(),
        None,
        "human_interview turn must not acquire LSM execution authority"
    );
    store
        .mark_launch_running(first.id, Some(101), "stdout-1".into(), "stderr-1".into())
        .await
        .unwrap();
    store
        .agent_reply_task_thread(
            thread_id,
            agent.id,
            "Which compatibility behavior should I preserve?",
        )
        .await
        .unwrap();
    store
        .finish_launch_attempt(first.id, Some(0), Some("provider-session".into()), None)
        .await
        .unwrap();
    assert_eq!(
        store.get_assignment(assignment_id).await.unwrap().status,
        "active",
        "finishing an intake turn must retain Assignment ownership"
    );

    store
        .create_human_task_message(
            task.id,
            agent.id,
            "Preserve operator recovery behavior.",
            None,
        )
        .await
        .unwrap();
    assert!(
        store
            .intake_delivery_resume_candidates()
            .await
            .unwrap()
            .contains(&assignment_id)
    );
    let second = store
        .enqueue_intake_continuation(assignment_id)
        .await
        .unwrap();
    assert_eq!(second.resume_from_attempt_id, Some(first.id));
    let second_execution = store
        .begin_launch_attempt_with_local_compat(second.id, Some("test-lsm-subject"))
        .await
        .unwrap();
    assert_eq!(
        store
            .run_runtime_provisioning_subject(second_execution.run.id)
            .await
            .unwrap(),
        None
    );
    store
        .mark_launch_running(second.id, Some(102), "stdout-2".into(), "stderr-2".into())
        .await
        .unwrap();
    let final_summary = store
        .agent_reply_task_thread(
            thread_id,
            agent.id,
            "Final synthesis: preserve operator recovery behavior; inspect, implement, and run acceptance tests.",
        )
        .await
        .unwrap();
    store
        .finish_launch_attempt(second.id, Some(0), Some("provider-session".into()), None)
        .await
        .unwrap();

    let confirmation = store
        .create_human_task_message(task.id, agent.id, "没问题，按这个做。", None)
        .await
        .unwrap();
    let third = store
        .enqueue_intake_continuation(assignment_id)
        .await
        .unwrap();
    assert_eq!(third.resume_from_attempt_id, Some(second.id));
    let third_execution = store
        .begin_launch_attempt_with_local_compat(third.id, Some("test-lsm-subject"))
        .await
        .unwrap();
    assert_eq!(
        store
            .run_runtime_provisioning_subject(third_execution.run.id)
            .await
            .unwrap(),
        None
    );
    store
        .mark_launch_running(third.id, Some(103), "stdout-3".into(), "stderr-3".into())
        .await
        .unwrap();
    store
        .finalize_intake_interview(
            task.id,
            agent.id,
            finalize(final_summary.id, confirmation.id),
        )
        .await
        .unwrap();
    assert_eq!(
        store.get_assignment(assignment_id).await.unwrap().phase,
        INTAKE_PHASE_READY
    );
    store
        .finish_launch_attempt(third.id, Some(0), Some("provider-session".into()), None)
        .await
        .unwrap();

    assert!(
        store
            .intake_delivery_resume_candidates()
            .await
            .unwrap()
            .contains(&assignment_id),
        "converged ready intake should automatically queue the implementation runtime"
    );
    let implementation = store
        .enqueue_intake_continuation(assignment_id)
        .await
        .unwrap();
    assert_eq!(
        implementation.resume_from_attempt_id,
        Some(third.id),
        "implementation must resume the latest Provider conversation without a Morrows Session identity"
    );
    let implementation_execution = store
        .begin_launch_attempt_with_local_compat(implementation.id, Some("test-lsm-subject"))
        .await
        .unwrap();
    assert_eq!(
        store.get_assignment(assignment_id).await.unwrap().phase,
        INTAKE_PHASE_IMPLEMENTING
    );
    assert_eq!(
        store
            .run_runtime_provisioning_subject(implementation_execution.run.id)
            .await
            .unwrap()
            .as_deref(),
        Some("test-lsm-subject"),
        "only the implementation launch may create LSM provisioning intent"
    );

    store
        .mark_launch_running(
            implementation.id,
            Some(104),
            "stdout-implementation".into(),
            "stderr-implementation".into(),
        )
        .await
        .unwrap();
    let artifact = store
        .create_artifact(
            task.id,
            agent.id,
            input(json!({
                "title":"E2E acceptance evidence",
                "uri":"simulation://session-dedup-e2e"
            })),
        )
        .await
        .unwrap();
    let preview = store
        .run_completion_check(implementation_execution.run.id, agent.id, &json!({}))
        .await
        .unwrap();
    let mut completion = preview.completion_template;
    for check in completion["checks"].as_array_mut().unwrap() {
        check["status"] = json!("passed");
        check["rationale"] = json!("The no-Session lifecycle E2E completed successfully.");
        check["artifact_ids"] = json!([artifact.id]);
    }
    store
        .complete_run(
            implementation_execution.run.id,
            agent.id,
            json!({
                "completion":completion,
                "memory_disposition":{
                    "status":"not_applicable",
                    "rationale":"This fixture validates lifecycle plumbing only; it creates no reusable Project Memory.",
                    "memory_entry_ids":[]
                },
                "simulation_only":true
            }),
        )
        .await
        .unwrap();
    store
        .finish_launch_attempt(
            implementation.id,
            Some(0),
            Some("provider-session".into()),
            None,
        )
        .await
        .unwrap();

    let completed_run = store
        .get_run(implementation_execution.run.id)
        .await
        .unwrap();
    assert_eq!(completed_run.status, "completed");
    assert_eq!(
        completed_run.provider_conversation_ref.as_deref(),
        Some("provider-session")
    );
    assert_eq!(
        store.get_task(task.id).await.unwrap().state,
        TaskState::Done
    );
    assert_eq!(
        store
            .task_agent_thread(task.id, agent.id)
            .await
            .unwrap()
            .unwrap()
            .id,
        thread_id,
        "Human/Agent transcript remains Task collaboration after completion"
    );
}
