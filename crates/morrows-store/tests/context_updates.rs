use morrows_core::{CreateContextRevision, UpdateContextRevision};
use morrows_store::Store;
use serde_json::json;

#[tokio::test]
async fn concurrent_partial_updates_preserve_each_others_fields_and_original_evidence() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let task = store
        .create_task(serde_json::from_value(json!({"title":"Audit"})).unwrap())
        .await
        .unwrap();
    let original = store.create_context_revision(task.id, serde_json::from_value(json!({
        "goal":"Freeze only after verification", "background":"Original receipts remain authoritative",
        "constraints":{"freeze_requires":["canary passes"],"limits":{"model_experiment":false,"budget":10}},
        "current_summary":"Audit incomplete"
    })).unwrap()).await.unwrap();
    let summary = UpdateContextRevision {
        current_summary: Some("Still incomplete; receipt located".into()),
        created_by_actor_id: "agent:summary-writer".into(),
        ..Default::default()
    };
    let constraint = UpdateContextRevision {
        constraints: Some(
            serde_json::from_value(json!({"limits":{"budget":20},"targeted_recheck":true}))
                .unwrap(),
        ),
        created_by_actor_id: "agent:constraint-writer".into(),
        ..Default::default()
    };
    let (a, b) = tokio::join!(
        store.update_context_revision(task.id, summary),
        store.update_context_revision(task.id, constraint)
    );
    a.unwrap();
    b.unwrap();
    let current = store.get_current_context(task.id).await.unwrap();
    assert_eq!(current.version, 3);
    assert_eq!(current.goal, original.goal);
    assert_eq!(current.background, original.background);
    assert_eq!(current.current_summary, "Still incomplete; receipt located");
    assert_eq!(
        current.constraints,
        json!({"freeze_requires":["canary passes"],"limits":{"model_experiment":false,"budget":20},"targeted_recheck":true})
    );
    assert_eq!(
        store
            .get_context_revision(original.id)
            .await
            .unwrap()
            .constraints,
        original.constraints
    );
}

#[tokio::test]
async fn stale_and_empty_updates_do_not_create_revisions_or_change_task_state() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let task = store
        .create_task(serde_json::from_value(json!({"title":"Audit"})).unwrap())
        .await
        .unwrap();
    let first = store
        .create_context_revision(
            task.id,
            serde_json::from_value(json!({"goal":"Original"})).unwrap(),
        )
        .await
        .unwrap();
    let second = store
        .update_context_revision(
            task.id,
            UpdateContextRevision {
                current_summary: Some("New evidence".into()),
                expected_context_revision_id: Some(first.id),
                created_by_actor_id: "agent:writer".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let before = store.get_task(task.id).await.unwrap();
    let events = store.task_events(task.id).await.unwrap().len();
    let stale = store
        .update_context_revision(
            task.id,
            UpdateContextRevision {
                current_summary: Some("Stale inference".into()),
                expected_context_revision_id: Some(first.id),
                created_by_actor_id: "agent:writer".into(),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(stale.to_string().contains("context changed"));
    assert!(
        store
            .update_context_revision(task.id, UpdateContextRevision::default())
            .await
            .is_err()
    );
    assert_eq!(
        store.get_current_context(task.id).await.unwrap().id,
        second.id
    );
    assert_eq!(
        store.get_task(task.id).await.unwrap().updated_at,
        before.updated_at
    );
    assert_eq!(store.task_events(task.id).await.unwrap().len(), events);
    assert_eq!(
        store
            .context_revisions_page(task.id, 20, 0)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
}

#[tokio::test]
async fn legacy_constraint_values_survive_summary_updates_and_full_replacement_is_explicit() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let task = store
        .create_task(serde_json::from_value(json!({"title":"Legacy"})).unwrap())
        .await
        .unwrap();
    store
        .create_context_revision(
            task.id,
            serde_json::from_value(json!({"constraints":["Preserve this legacy rule"]})).unwrap(),
        )
        .await
        .unwrap();
    let updated = store
        .update_context_revision(
            task.id,
            UpdateContextRevision {
                current_summary: Some("A progress update".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(updated.constraints, json!(["Preserve this legacy rule"]));
    assert!(
        store
            .update_context_revision(
                task.id,
                UpdateContextRevision {
                    constraints: Some(serde_json::from_value(json!({"rule":"new"})).unwrap()),
                    ..Default::default()
                }
            )
            .await
            .is_err()
    );
    let replacement = store
        .create_context_revision(
            task.id,
            CreateContextRevision {
                goal: "Explicit operator replacement".into(),
                background: String::new(),
                constraints: json!({}),
                current_summary: String::new(),
                created_by_actor_id: "human:test".into(),
            },
        )
        .await
        .unwrap();
    assert_eq!(replacement.constraints, json!({}));
    assert!(replacement.current_summary.is_empty());
}
