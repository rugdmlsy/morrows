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

fn submission(questions: Vec<&str>) -> InterviewSubmission {
    InterviewSubmission {
        understanding: "Read current project/task context before implementation.".into(),
        constraints: json!({"preserve_existing_behavior":true}),
        plan: json!([
            "inspect current state",
            "implement only after human approval",
            "verify acceptance criteria"
        ]),
        questions: questions.into_iter().map(str::to_owned).collect(),
    }
}

#[tokio::test]
async fn open_claim_requires_current_reads_and_human_approval_before_implementation() {
    let (store, _project, agent, task) = fixture().await;
    let claim = store
        .claim_task_for_execution(task.id, agent.id, "executor", 300)
        .await
        .unwrap();
    assert_eq!(claim.assignment.phase, INTAKE_PHASE_CONTEXT_REVIEW);
    assert_eq!(claim.run.status, "running");
    let initial = store
        .get_assignment_intake(claim.assignment.id)
        .await
        .unwrap();
    assert_eq!(initial.interview_status, "not_started");

    let blocked = store
        .begin_task_execution(task.id, agent.id)
        .await
        .unwrap_err();
    assert!(blocked.to_string().contains("context_review"));

    let view = store
        .task_intake_page(task.id, agent.id, 100, 0)
        .await
        .unwrap();
    assert!(view.project_memory.next_offset.is_none());
    assert!(view.intake.project_memory_complete);
    assert!(view.intake.context_package_id.is_some());
    assert!(!view.execution_ready);
    assert!(
        view.blockers
            .iter()
            .any(|b| b == "human_interview_not_approved")
    );

    let pending = store
        .submit_intake_interview(
            task.id,
            agent.id,
            submission(vec!["Which compatibility behavior is required?"]),
        )
        .await
        .unwrap();
    assert_eq!(pending.interview_status, "pending");
    assert_eq!(
        store
            .get_assignment(claim.assignment.id)
            .await
            .unwrap()
            .phase,
        INTAKE_PHASE_HUMAN_INTERVIEW
    );

    let empty_approval = store
        .resolve_intake_interview(claim.assignment.id, "approve", "", "human:test")
        .await
        .unwrap_err();
    assert!(
        empty_approval
            .to_string()
            .contains("approval response is required")
    );

    let revised = store
        .resolve_intake_interview(
            claim.assignment.id,
            "revise",
            "Preserve existing operator recovery behavior; resubmit your plan.",
            "human:test",
        )
        .await
        .unwrap();
    assert_eq!(revised.interview_status, "revision_requested");
    let cannot_skip_resubmit = store
        .resolve_intake_interview(claim.assignment.id, "approve", "approved", "human:test")
        .await
        .unwrap_err();
    assert!(
        cannot_skip_resubmit
            .to_string()
            .contains("must submit or resubmit")
    );

    store
        .submit_intake_interview(task.id, agent.id, submission(vec![]))
        .await
        .unwrap();
    store
        .resolve_intake_interview(claim.assignment.id, "approve", "", "human:test")
        .await
        .unwrap();
    assert_eq!(
        store
            .get_assignment(claim.assignment.id)
            .await
            .unwrap()
            .phase,
        INTAKE_PHASE_READY
    );

    store
        .create_context_revision(
            task.id,
            input(json!({
                "goal":"ship safely",
                "background":"changed after human approval",
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
    assert_eq!(refreshed.intake.interview_status, "not_started");
    assert!(refreshed.intake.approved_at.is_none());

    store
        .submit_intake_interview(task.id, agent.id, submission(vec![]))
        .await
        .unwrap();
    store
        .resolve_intake_interview(claim.assignment.id, "approve", "", "human:test")
        .await
        .unwrap();
    let implementing = store.begin_task_execution(task.id, agent.id).await.unwrap();
    assert_eq!(implementing.phase, INTAKE_PHASE_IMPLEMENTING);
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
