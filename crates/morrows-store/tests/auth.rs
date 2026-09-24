use morrows_core::*;
use morrows_store::Store;
use serde_json::{Value, json};

fn input<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn bridge_credential_persists_only_hash_and_revocation_is_immediate() {
    let path = std::env::temp_dir().join(format!("morrows-auth-{}.db", Id::new_v4()));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let store = Store::connect(&url).await.unwrap();
    let agent = store.register_agent("credential-agent", &[]).await.unwrap();

    let issued = store
        .issue_bridge_credential(agent.id, "test bridge", 600)
        .await
        .unwrap();
    assert!(issued.token.starts_with("mrw_agent_"));
    assert_eq!(
        store
            .verify_agent_token(&issued.token)
            .await
            .unwrap()
            .agent_instance_id,
        agent.id
    );

    let listed = store.list_agent_credentials(agent.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    let listed_json = serde_json::to_string(&listed).unwrap();
    assert!(!listed_json.contains(&issued.token));

    store
        .revoke_agent_credential(issued.credential.id)
        .await
        .unwrap();
    assert!(store.verify_agent_token(&issued.token).await.is_err());

    drop(store);
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    for candidate in [
        path.clone(),
        path.with_extension("db-wal"),
        path.with_extension("db-shm"),
    ] {
        if let Ok(bytes) = std::fs::read(&candidate) {
            assert!(
                !bytes
                    .windows(issued.token.len())
                    .any(|window| window == issued.token.as_bytes()),
                "plaintext Agent credential leaked into {}",
                candidate.display()
            );
        }
        let _ = std::fs::remove_file(candidate);
    }
}

#[tokio::test]
async fn finishing_launch_revokes_runtime_credential_for_the_run() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store
        .register_agent("runtime-auth-agent", &[])
        .await
        .unwrap();
    let task = store
        .create_task(input(json!({"title":"runtime auth"})))
        .await
        .unwrap();
    let assignment = store
        .claim_task(task.id, agent.id, "executor", 600)
        .await
        .unwrap();
    let profile = store
        .register_launch_profile(input(json!({
            "name":"runtime auth codex",
            "adapter":"codex_cli",
            "agent_instance_id":agent.id,
            "program":"/bin/echo",
            "default_cwd":"/tmp"
        })))
        .await
        .unwrap();
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment.id,
            "launch_profile_id":profile.id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();

    let issued = store
        .issue_runtime_credential(agent.id, execution.run.id, "runtime test", 600)
        .await
        .unwrap();
    assert!(store.verify_agent_token(&issued.token).await.is_ok());

    store
        .finish_launch_attempt(attempt.id, Some(0), Some("session-auth".into()), None)
        .await
        .unwrap();
    assert!(store.verify_agent_token(&issued.token).await.is_err());
    assert!(
        store
            .get_agent_credential(issued.credential.id)
            .await
            .unwrap()
            .revoked_at
            .is_some()
    );
}
