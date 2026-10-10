use morrows_core::{CreateProject, CreateTask};
use morrows_store::Store;

#[tokio::test]
async fn projects_group_tasks_and_task_can_move_between_projects() {
    let store = Store::connect("sqlite::memory:").await.unwrap();

    let alpha = store
        .create_project(CreateProject {
            name: "Alpha".into(),
            description: "First project".into(),
        })
        .await
        .unwrap();
    let beta = store
        .create_project(CreateProject {
            name: "Beta".into(),
            description: "Second project".into(),
        })
        .await
        .unwrap();

    let task = store
        .create_task(CreateTask {
            acceptance_criteria: vec![],
            project_id: Some(alpha.id),
            title: "Grouped task".into(),
            description: "metadata".into(),
            owner_actor_id: "human:test".into(),
            state: morrows_core::TaskState::Ready,
            priority: 7,
        })
        .await
        .unwrap();

    assert_eq!(task.project_id, Some(alpha.id));
    assert_eq!(store.list_projects().await.unwrap().len(), 2);

    let moved = store
        .set_task_project(task.id, Some(beta.id))
        .await
        .unwrap();
    assert_eq!(moved.project_id, Some(beta.id));

    let unclassified = store.set_task_project(task.id, None).await.unwrap();
    assert_eq!(unclassified.project_id, None);

    let events = store.task_events(task.id).await.unwrap();
    assert!(
        events
            .iter()
            .filter(|event| event.event_type == "task.project_changed")
            .count()
            >= 2
    );
}

#[tokio::test]
async fn task_management_summary_includes_project_executor_and_latest_run() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Managed".into(),
            description: "Task overview".into(),
        })
        .await
        .unwrap();
    let agent = store
        .register_agent("task-manager-worker", &["code".into()])
        .await
        .unwrap();
    let task = store
        .create_task(CreateTask {
            acceptance_criteria: vec![],
            project_id: Some(project.id),
            title: "Managed task".into(),
            description: "metadata".into(),
            owner_actor_id: "human:test".into(),
            state: morrows_core::TaskState::Ready,
            priority: 9,
        })
        .await
        .unwrap();
    let assignment = store
        .claim_task(task.id, agent.id, "executor", 300)
        .await
        .unwrap();
    let run = store
        .start_run(assignment.id, agent.id, None)
        .await
        .unwrap();

    let rows = store.list_task_management().await.unwrap();
    let row = rows.iter().find(|row| row.task.id == task.id).unwrap();
    assert_eq!(row.project_name.as_deref(), Some("Managed"));
    assert_eq!(row.executor_assignment_id, Some(assignment.id));
    assert_eq!(row.executor_assignment_status.as_deref(), Some("active"));
    assert_eq!(row.executor_agent_instance_id, Some(agent.id));
    assert_eq!(
        row.executor_agent_display_name.as_deref(),
        Some(agent.display_name.as_str())
    );
    assert_eq!(row.latest_run_id, Some(run.id));
    assert_eq!(row.latest_run_status.as_deref(), Some("running"));
    assert!(row.executor_acquired_at.is_some());
    assert!(row.latest_run_started_at.is_some());
}

#[tokio::test]
async fn task_rejects_unknown_project() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let before = store.list_tasks().await.unwrap().len();
    let err = store
        .create_task(CreateTask {
            acceptance_criteria: vec![],
            project_id: Some(uuid::Uuid::new_v4()),
            title: "bad project".into(),
            description: String::new(),
            owner_actor_id: "human:test".into(),
            state: morrows_core::TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("project"));
    assert_eq!(store.list_tasks().await.unwrap().len(), before);
}

#[tokio::test]
async fn failed_project_repair_is_atomic_and_preserves_existing_binding() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Original".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let task = store
        .create_task(CreateTask {
            acceptance_criteria: vec![],
            project_id: Some(project.id),
            title: "Keep binding".into(),
            description: String::new(),
            owner_actor_id: "human:test".into(),
            state: morrows_core::TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap();
    let event_count = store.task_events(task.id).await.unwrap().len();

    let missing = uuid::Uuid::new_v4();
    assert!(
        store
            .set_task_project(task.id, Some(missing))
            .await
            .unwrap_err()
            .to_string()
            .contains("project")
    );
    assert_eq!(
        store.get_task(task.id).await.unwrap().project_id,
        Some(project.id)
    );
    assert_eq!(store.task_events(task.id).await.unwrap().len(), event_count);
}

#[tokio::test]
async fn unbound_project_repair_is_atomic_idempotent_and_refuses_reassignment() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let alpha = store
        .create_project(CreateProject {
            name: "Alpha repair".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let beta = store
        .create_project(CreateProject {
            name: "Beta repair".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let task = store
        .create_task(CreateTask {
            acceptance_criteria: vec![],
            project_id: None,
            title: "Legacy unbound".into(),
            description: String::new(),
            owner_actor_id: "agent:test".into(),
            state: morrows_core::TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap();

    let repaired = store
        .bind_unbound_task_project(task.id, alpha.id)
        .await
        .unwrap();
    assert_eq!(repaired.id, task.id);
    assert_eq!(repaired.project_id, Some(alpha.id));
    let events_after_first = store.task_events(task.id).await.unwrap();
    assert!(
        events_after_first
            .iter()
            .any(|event| event.event_type == "task.project_changed"
                && event.payload["reason"] == "legacy_unbound_repair")
    );

    store
        .bind_unbound_task_project(task.id, alpha.id)
        .await
        .unwrap();
    assert_eq!(
        store.task_events(task.id).await.unwrap().len(),
        events_after_first.len(),
        "same-target retry must be idempotent"
    );

    let error = store
        .bind_unbound_task_project(task.id, beta.id)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("refuses reassignment"));
    assert_eq!(
        store.get_task(task.id).await.unwrap().project_id,
        Some(alpha.id)
    );
}

#[tokio::test]
async fn pristine_task_can_be_deleted_but_task_with_history_is_preserved() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let pristine = store
        .create_task(CreateTask {
            acceptance_criteria: vec![],
            project_id: None,
            title: "Disposable task".into(),
            description: String::new(),
            owner_actor_id: "human:webui".into(),
            state: morrows_core::TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap();
    store.delete_task(pristine.id).await.unwrap();
    assert!(store.get_task(pristine.id).await.is_err());

    let retained = store
        .create_task(CreateTask {
            acceptance_criteria: vec![],
            project_id: None,
            title: "Task with context".into(),
            description: String::new(),
            owner_actor_id: "human:webui".into(),
            state: morrows_core::TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap();
    store
        .create_context_revision(
            retained.id,
            morrows_core::CreateContextRevision {
                goal: "keep history".into(),
                background: String::new(),
                constraints: serde_json::json!({}),
                current_summary: String::new(),
                created_by_actor_id: "human:webui".into(),
            },
        )
        .await
        .unwrap();
    let error = store.delete_task(retained.id).await.unwrap_err();
    assert!(error.to_string().contains("cannot be hard-deleted"));
    assert_eq!(store.get_task(retained.id).await.unwrap().id, retained.id);
}
