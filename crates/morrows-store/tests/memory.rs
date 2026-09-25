use morrows_core::{CreateMemoryEntry, CreateProject, CreateTask};
use morrows_store::Store;
use serde_json::json;

#[tokio::test]
async fn long_term_memory_scopes_materialize_for_task() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store.register_agent("memory-agent", &[]).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Memory Project".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let task = store
        .create_task(CreateTask {
            project_id: Some(project.id),
            title: "Use memory".into(),
            description: String::new(),
            owner_actor_id: "human:test".into(),
            state: morrows_core::TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap();

    let organization = store
        .create_memory_entry(CreateMemoryEntry {
            scope_type: "organization".into(),
            project_id: None,
            agent_instance_id: None,
            task_id: None,
            title: "Org rule".into(),
            content: json!({"rule":"canonical source of truth"}),
            source_kind: "manual".into(),
            source_ref: None,
            visibility: "shared".into(),
            supersedes_memory_id: None,
        })
        .await
        .unwrap();

    let project_memory = store
        .create_memory_entry(CreateMemoryEntry {
            scope_type: "project".into(),
            project_id: Some(project.id),
            agent_instance_id: None,
            task_id: None,
            title: "Project architecture".into(),
            content: json!({"fact":"project-scoped"}),
            source_kind: "chatgpt_history".into(),
            source_ref: Some("history:test".into()),
            visibility: "shared".into(),
            supersedes_memory_id: None,
        })
        .await
        .unwrap();

    let agent_private = store
        .create_memory_entry(CreateMemoryEntry {
            scope_type: "agent".into(),
            project_id: None,
            agent_instance_id: Some(agent.id),
            task_id: None,
            title: "Agent preference".into(),
            content: json!({"preference":"concise"}),
            source_kind: "manual".into(),
            source_ref: None,
            visibility: "private".into(),
            supersedes_memory_id: None,
        })
        .await
        .unwrap();

    let task_memory = store
        .create_memory_entry(CreateMemoryEntry {
            scope_type: "task".into(),
            project_id: None,
            agent_instance_id: None,
            task_id: Some(task.id),
            title: "Task fact".into(),
            content: json!({"fact":"task-scoped"}),
            source_kind: "system".into(),
            source_ref: None,
            visibility: "shared".into(),
            supersedes_memory_id: None,
        })
        .await
        .unwrap();

    let materialized = store.memories_for_task(task.id, agent.id).await.unwrap();
    let ids = materialized
        .iter()
        .map(|entry| entry.id)
        .collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![
            organization.id,
            project_memory.id,
            agent_private.id,
            task_memory.id,
        ]
    );
}

#[tokio::test]
async fn memory_supersede_requires_same_scope() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "P".into(),
            description: String::new(),
        })
        .await
        .unwrap();
    let first = store
        .create_memory_entry(CreateMemoryEntry {
            scope_type: "project".into(),
            project_id: Some(project.id),
            agent_instance_id: None,
            task_id: None,
            title: "v1".into(),
            content: json!({"version":1}),
            source_kind: "manual".into(),
            source_ref: None,
            visibility: "shared".into(),
            supersedes_memory_id: None,
        })
        .await
        .unwrap();

    let second = store
        .create_memory_entry(CreateMemoryEntry {
            scope_type: "project".into(),
            project_id: Some(project.id),
            agent_instance_id: None,
            task_id: None,
            title: "v2".into(),
            content: json!({"version":2}),
            source_kind: "manual".into(),
            source_ref: None,
            visibility: "shared".into(),
            supersedes_memory_id: Some(first.id),
        })
        .await
        .unwrap();
    assert_eq!(second.supersedes_memory_id, Some(first.id));

    let err = store
        .create_memory_entry(CreateMemoryEntry {
            scope_type: "organization".into(),
            project_id: None,
            agent_instance_id: None,
            task_id: None,
            title: "bad".into(),
            content: json!({}),
            source_kind: "manual".into(),
            source_ref: None,
            visibility: "shared".into(),
            supersedes_memory_id: Some(first.id),
        })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("same scope"));
}
