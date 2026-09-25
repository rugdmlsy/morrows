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
async fn task_rejects_unknown_project() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let err = store
        .create_task(CreateTask {
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
}
