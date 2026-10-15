use morrows_store::Store;
use serde_json::json;

#[tokio::test]
async fn project_details_show_only_current_shared_project_memory_with_pagination() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = store
        .create_project(serde_json::from_value(json!({"name":"A"})).unwrap())
        .await
        .unwrap();
    let b = store
        .create_project(serde_json::from_value(json!({"name":"B"})).unwrap())
        .await
        .unwrap();
    let agent = store.register_agent("test", &[]).await.unwrap();
    let insert = |project_id, title: &str, supersedes: Option<uuid::Uuid>| {
        let title = title.to_owned();
        let store = store.clone();
        async move {
            store
                .create_memory_entry(
                    serde_json::from_value(json!({
                        "scope_type":"project","project_id":project_id,"title":title,
                        "content":{"details":"test"},"source_kind":"manual",
                        "supersedes_memory_id":supersedes,
                        "visibility":"shared"
                    }))
                    .unwrap(),
                )
                .await
                .unwrap()
        }
    };
    let original = insert(a.id, "old rule", None).await;
    insert(a.id, "current rule", Some(original.id)).await;
    insert(a.id, "another current rule", None).await;
    insert(b.id, "other project", None).await;
    store.create_memory_entry(serde_json::from_value(json!({
        "scope_type":"organization","title":"org-wide","content":"hidden here","source_kind":"manual"
    })).unwrap()).await.unwrap();
    store
        .create_memory_entry(
            serde_json::from_value(json!({
                "scope_type":"agent","agent_instance_id":agent.id,"title":"private",
                "content":"secret","visibility":"private","source_kind":"manual"
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let first = store.project_memories_page(a.id, 1, 0).await.unwrap();
    assert_eq!(first.items.len(), 1);
    assert_eq!(first.next_offset, Some(1));
    let second = store.project_memories_page(a.id, 1, 1).await.unwrap();
    assert_eq!(second.items.len(), 1);
    assert_eq!(second.next_offset, None);
    let titles: Vec<_> = first
        .items
        .iter()
        .chain(second.items.iter())
        .map(|entry| entry.title.as_str())
        .collect();
    assert!(titles.contains(&"current rule"));
    assert!(titles.contains(&"another current rule"));
    assert!(!titles.contains(&"old rule"));
    assert_eq!(
        store
            .project_memories_page(b.id, 30, 0)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert!(store.project_memories_page(a.id, 0, 0).await.is_err());
    assert!(store.project_memories_page(a.id, 30, -1).await.is_err());
}
