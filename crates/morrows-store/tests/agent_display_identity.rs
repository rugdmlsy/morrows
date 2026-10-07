use morrows_core::{RegisterAccount, RegisterAgentInstance, RegisterMachine, RegisterProfile};
use morrows_store::Store;
use serde_json::json;

#[tokio::test]
async fn agent_display_names_are_numbered_and_survive_reregistration() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let profile = store
        .register_profile(
            serde_json::from_value::<RegisterProfile>(json!({
                "name": "Codex CLI",
                "provider": "openai",
                "kind": "coding_agent",
                "default_capabilities": ["code"]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let account = store
        .register_account(
            serde_json::from_value::<RegisterAccount>(json!({
                "provider": "openai",
                "label": "personal",
                "email": "person@example.com"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let machine = store
        .register_machine(
            serde_json::from_value::<RegisterMachine>(json!({
                "name": "mac",
                "hostname": "mac.local",
                "os": "macOS",
                "arch": "arm64"
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let first_input = serde_json::from_value::<RegisterAgentInstance>(json!({
        "profile_id": profile.id,
        "account_id": account.id,
        "machine_id": machine.id,
        "name": "codex-runtime-a",
        "external_instance_ref": "codex-a"
    }))
    .unwrap();
    let first = store
        .register_agent_instance(first_input.clone())
        .await
        .unwrap();
    let second = store
        .register_agent_instance(
            serde_json::from_value::<RegisterAgentInstance>(json!({
                "profile_id": profile.id,
                "account_id": account.id,
                "machine_id": machine.id,
                "name": "codex-runtime-b",
                "external_instance_ref": "codex-b"
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(first.display_name, "codex-0");
    assert_eq!(second.display_name, "codex-1");
    assert_eq!(account.email.as_deref(), Some("person@example.com"));

    let renamed = store
        .rename_agent(first.id, "research-codex")
        .await
        .unwrap();
    assert_eq!(renamed.name, "codex-runtime-a");
    assert_eq!(renamed.display_name, "research-codex");

    let refreshed = store.register_agent_instance(first_input).await.unwrap();
    assert_eq!(refreshed.id, first.id);
    assert_eq!(refreshed.display_name, "research-codex");
}

#[tokio::test]
async fn archived_agents_leave_fleet_without_losing_identity() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let profile = store
        .register_profile(
            serde_json::from_value::<RegisterProfile>(json!({
                "name": "CodeBuddy Code",
                "provider": "tencent",
                "kind": "coding_agent"
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let agent = store
        .register_agent_instance(
            serde_json::from_value::<RegisterAgentInstance>(json!({
                "profile_id": profile.id,
                "name": "codebuddy-runtime"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(agent.display_name, "codebuddy-0");
    assert_eq!(store.agent_fleet().await.unwrap().len(), 1);
    let task = store
        .create_task(serde_json::from_value(json!({"title":"identity task"})).unwrap())
        .await
        .unwrap();
    let message = store
        .create_human_task_message(task.id, agent.id, "identity survives archive", None)
        .await
        .unwrap();

    let archived = store.archive_agent(agent.id).await.unwrap();
    assert_eq!(archived.status, "archived");
    assert!(archived.archived_at.is_some());
    assert!(store.agent_fleet().await.unwrap().is_empty());
    assert_eq!(
        store.get_task_message(message.id).await.unwrap().body,
        "identity survives archive"
    );

    let still_there = store.get_agent(agent.id).await.unwrap();
    assert_eq!(still_there.name, "codebuddy-runtime");
}
