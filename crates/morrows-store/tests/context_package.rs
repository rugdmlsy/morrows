use morrows_store::Store;
use serde_json::json;

#[tokio::test]
async fn package_preserves_execution_background_and_all_shared_memory_references() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store.register_agent("package-writer", &[]).await.unwrap();
    let project = store
        .create_project(
            serde_json::from_value(json!({
                "name":"Audit", "description":"Why this audit matters"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let task = store
        .create_task(
            serde_json::from_value(json!({"title":"Finish audit","project_id":project.id}))
                .unwrap(),
        )
        .await
        .unwrap();
    let background = format!(
        "{}\nMachine: test-node; workspace: /srv/work/audit; report: work/report.json; source: authorized-history; observed_at: 2026-09-25; filesystem_revalidated: false",
        "Preserve historical evidence. ".repeat(500)
    );
    let context = store
        .create_context_revision(
            task.id,
            serde_json::from_value(json!({
                "goal":"Resolve audit gaps", "background":background,
                "constraints":{"freeze_requires":["recheck passes"]},
                "current_summary":"Read original receipts before targeted recheck."
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    for i in 0..103 {
        store.create_memory_entry(serde_json::from_value(json!({
            "scope_type":"organization","title":format!("Rule {i}"),"content":"Keep evidence","source_kind":"test"
        })).unwrap()).await.unwrap();
    }
    let old = store.create_memory_entry(serde_json::from_value(json!({
        "scope_type":"project","project_id":project.id,"title":"Old rule","content":"Old","source_kind":"test"
    })).unwrap()).await.unwrap();
    let current = store.create_memory_entry(serde_json::from_value(json!({
        "scope_type":"project","project_id":project.id,"title":"Current rule","content":"Current","source_kind":"test","supersedes_memory_id":old.id
    })).unwrap()).await.unwrap();
    let private = store.create_memory_entry(serde_json::from_value(json!({
        "scope_type":"agent","agent_instance_id":agent.id,"title":"Private","content":"Private credential reference","source_kind":"test","visibility":"private"
    })).unwrap()).await.unwrap();
    let package = store
        .assemble_context_package(task.id, None, Some(agent.id))
        .await
        .unwrap();
    let summary = package.summary.as_ref().unwrap();
    assert_eq!(package.context_snapshot_id, Some(context.id));
    assert_eq!(summary["context"]["background"], background);
    assert_eq!(
        summary["context"]["constraints"]["freeze_requires"],
        json!(["recheck passes"])
    );
    assert_eq!(summary["project"]["description"], "Why this audit matters");
    assert_eq!(package.memory_refs.len(), 104);
    assert!(package.memory_refs.contains(&current.id.to_string()));
    assert!(!package.memory_refs.contains(&old.id.to_string()));
    assert!(!package.memory_refs.contains(&private.id.to_string()));
    assert_eq!(package.next_action, context.current_summary);
    assert_eq!(summary["next_action_source"], "context.current_summary");
    assert!(package.verified_results.is_empty());
    assert_eq!(
        store.get_context_package(package.id).await.unwrap().summary,
        package.summary
    );
}
