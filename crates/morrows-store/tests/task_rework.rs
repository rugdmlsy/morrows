use morrows_core::*;
use morrows_store::Store;
use serde_json::json;

fn task_input(title: &str, state: TaskState, project_id: Option<Id>, priority: i32) -> CreateTask {
    CreateTask {
        acceptance_criteria: vec![],
        project_id,
        title: title.into(),
        description: String::new(),
        owner_actor_id: "human:test".into(),
        state,
        priority,
    }
}

#[tokio::test]
async fn rework_is_a_new_task_and_preserves_completed_source() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Morrows".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let source = store
        .create_task(task_input(
            "Finished feature",
            TaskState::Done,
            Some(project.id),
            7,
        ))
        .await
        .unwrap();
    store
        .set_task_assignment_mode(source.id, AssignmentMode::Approval)
        .await
        .unwrap();

    let (first, relation) = store
        .create_rework_task(
            source.id,
            "human:webui".into(),
            "human:webui".into(),
            Some("Fix regression".into()),
            None,
            "Regression found after completion".into(),
        )
        .await
        .unwrap();

    assert_eq!(
        store.get_task(source.id).await.unwrap().state,
        TaskState::Done
    );
    assert_eq!(first.state, TaskState::Ready);
    assert_eq!(first.project_id, Some(project.id));
    assert_eq!(first.assignment_mode, AssignmentMode::Approval);
    assert_eq!(first.priority, 7);
    assert_eq!(relation.source_task_id, first.id);
    assert_eq!(relation.target_task_id, source.id);
    assert_eq!(relation.relation_type, "rework_of");
    assert_eq!(relation.metadata["root_task_id"], json!(source.id));
    assert_eq!(relation.metadata["rework_round"], 1);
    assert_eq!(
        relation.metadata["reason"],
        "Regression found after completion"
    );

    let (second, second_relation) = store
        .create_rework_task(
            source.id,
            "human:webui".into(),
            "human:webui".into(),
            None,
            None,
            "Another post-completion issue".into(),
        )
        .await
        .unwrap();
    assert_ne!(first.id, second.id);
    assert_eq!(second_relation.metadata["rework_round"], 2);

    let source_relationships = store.task_relationships(source.id).await.unwrap();
    assert_eq!(
        source_relationships
            .iter()
            .filter(|item| item.relation_type == "rework_of")
            .count(),
        2
    );

    let package = store
        .assemble_context_package(first.id, None, None)
        .await
        .unwrap();
    let summary = package.summary.unwrap();
    assert_eq!(summary["rework_source"]["task"]["id"], json!(source.id));
    assert_eq!(
        summary["rework_source"]["relationship"]["metadata"]["reason"],
        "Regression found after completion"
    );
    assert!(
        package
            .next_action
            .contains("Regression found after completion")
    );
}

#[tokio::test]
async fn rework_context_package_carries_source_completion_evidence() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = store.register_agent("worker", &[]).await.unwrap();
    let source = store
        .create_task(task_input("Original execution", TaskState::Ready, None, 0))
        .await
        .unwrap();
    store
        .create_context_revision(
            source.id,
            CreateContextRevision {
                goal: "Ship original".into(),
                background: "Original background".into(),
                constraints: json!({}),
                current_summary: "Ready to finish".into(),
                created_by_actor_id: "human:test".into(),
            },
        )
        .await
        .unwrap();
    let artifact = store
        .create_artifact(
            source.id,
            worker.id,
            CreateArtifact {
                title: "Report".into(),
                uri: "file:///tmp/report.md".into(),
                kind: "report".into(),
                description: "Final report".into(),
            },
        )
        .await
        .unwrap();
    let decision = store
        .create_decision(
            source.id,
            worker.id,
            CreateDecision {
                title: "Keep implementation".into(),
                rationale: "Validated".into(),
            },
        )
        .await
        .unwrap();
    let claim = store
        .claim_task_for_execution(source.id, worker.id, "executor", 300)
        .await
        .unwrap();
    let milestone = store
        .create_run_milestone(
            claim.run.id,
            worker.id,
            CreateRunMilestone {
                kind: "milestone".into(),
                summary: "Original work complete".into(),
                completed: vec!["implementation".into()],
                verified: vec!["tests passed".into()],
                remaining: vec![],
                blockers: vec![],
                next_step: "Complete run".into(),
                next_plan: vec!["Complete run".into()],
                execution_locations: vec!["/work".into()],
                artifact_ids: vec![artifact.id.to_string()],
                decision_ids: vec![decision.id.to_string()],
            },
        )
        .await
        .unwrap();
    store
        .complete_run(
            claim.run.id,
            worker.id,
            json!({"summary":"done","report_path":"/tmp/report.md"}),
        )
        .await
        .unwrap();

    let (rework, _) = store
        .create_rework_task(
            source.id,
            "human:webui".into(),
            "human:webui".into(),
            None,
            None,
            "Acceptance review found a regression".into(),
        )
        .await
        .unwrap();
    let package = store
        .assemble_context_package(rework.id, None, None)
        .await
        .unwrap();
    let summary = package.summary.unwrap();

    assert_eq!(
        summary["rework_source"]["latest_completed_run"]["result"]["report_path"],
        "/tmp/report.md"
    );
    assert_eq!(
        summary["rework_source"]["latest_milestone"]["id"],
        json!(milestone.id)
    );
    assert_eq!(
        summary["rework_source"]["artifacts"][0]["id"],
        json!(artifact.id)
    );
    assert_eq!(
        summary["rework_source"]["decisions"][0]["id"],
        json!(decision.id)
    );
    assert!(package.artifact_refs.contains(&artifact.id));
    assert!(package.decision_refs.contains(&decision.id));
}

#[tokio::test]
async fn dispatch_rework_inherits_a_usable_dispatch_policy() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let source = store
        .create_task(task_input("Dispatched source", TaskState::Done, None, 4))
        .await
        .unwrap();
    let source_policy = store
        .set_dispatch_policy(
            source.id,
            SetDispatchPolicy {
                role: "executor".into(),
                required_capabilities: vec!["shell".into()],
                profile_id: None,
                account_id: None,
                machine_id: None,
                heartbeat_ttl_seconds: 300,
                capacity_ttl_seconds: 300,
                lease_seconds: 900,
                enabled: true,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.get_task(source.id).await.unwrap().assignment_mode,
        AssignmentMode::Dispatch
    );

    let (rework, relation) = store
        .create_rework_task(
            source.id,
            "human:webui".into(),
            "human:webui".into(),
            None,
            None,
            "Dispatch-managed work needs correction".into(),
        )
        .await
        .unwrap();
    assert_eq!(rework.assignment_mode, AssignmentMode::Dispatch);
    let inherited = store
        .get_dispatch_policy(rework.id, "executor")
        .await
        .unwrap();
    assert_eq!(
        inherited.required_capabilities,
        source_policy.required_capabilities
    );
    assert_eq!(inherited.lease_seconds, source_policy.lease_seconds);
    assert!(inherited.enabled);
    assert_eq!(
        relation.metadata["inherited_dispatch_policy_count"],
        json!(1)
    );

    let invalid = store
        .create_task(task_input(
            "Broken dispatch source",
            TaskState::Done,
            None,
            0,
        ))
        .await
        .unwrap();
    store
        .set_task_assignment_mode(invalid.id, AssignmentMode::Dispatch)
        .await
        .unwrap();
    let before = store.list_tasks().await.unwrap().len();
    let error = store
        .create_rework_task(
            invalid.id,
            "human:webui".into(),
            "human:webui".into(),
            None,
            None,
            "Should not create a stuck successor".into(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("no enabled dispatch policy"));
    assert_eq!(
        store.list_tasks().await.unwrap().len(),
        before,
        "failed dispatch inheritance must roll back the new task"
    );
}

#[tokio::test]
async fn rework_requires_done_and_reopen_is_narrow() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = store.register_agent("worker", &[]).await.unwrap();
    let ready = store
        .create_task(task_input("Not done", TaskState::Ready, None, 0))
        .await
        .unwrap();
    assert!(
        store
            .create_rework_task(
                ready.id,
                "human:webui".into(),
                "human:webui".into(),
                None,
                None,
                "too early".into(),
            )
            .await
            .unwrap_err()
            .to_string()
            .contains("completed task")
    );

    let mistaken = store
        .create_task(task_input("Mistaken completion", TaskState::Done, None, 0))
        .await
        .unwrap();
    let reopened = store
        .reopen_task(mistaken.id, "Completion was clicked by mistake".into())
        .await
        .unwrap();
    assert_eq!(reopened.state, TaskState::Ready);
    assert!(
        store
            .task_events(mistaken.id)
            .await
            .unwrap()
            .iter()
            .any(|event| event.event_type == "task.reopened")
    );

    let with_rework = store
        .create_task(task_input(
            "Completed with rework",
            TaskState::Done,
            None,
            0,
        ))
        .await
        .unwrap();
    store
        .create_rework_task(
            with_rework.id,
            "human:webui".into(),
            "human:webui".into(),
            None,
            None,
            "Real rework".into(),
        )
        .await
        .unwrap();
    assert!(
        store
            .reopen_task(with_rework.id, "try reopen".into())
            .await
            .unwrap_err()
            .to_string()
            .contains("rework history")
    );

    let prerequisite = store
        .create_task(task_input("Consumed completion", TaskState::Done, None, 0))
        .await
        .unwrap();
    let dependent = store
        .create_task(task_input("Downstream", TaskState::InProgress, None, 0))
        .await
        .unwrap();
    store
        .add_dependency(dependent.id, prerequisite.id, worker.id)
        .await
        .unwrap();
    assert!(
        store
            .reopen_task(prerequisite.id, "late correction".into())
            .await
            .unwrap_err()
            .to_string()
            .contains("consumed")
    );
}
