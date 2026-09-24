use morrows_store::Store;

#[tokio::test]
async fn operator_credentials_are_hash_only_and_revocable() {
    let path =
        std::env::temp_dir().join(format!("morrows-operator-auth-{}.db", uuid::Uuid::new_v4()));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let store = Store::connect(&url).await.unwrap();

    let issued = store
        .issue_operator_credential("test viewer", "viewer", 600)
        .await
        .unwrap();
    assert!(issued.token.starts_with("mrw_operator_"));
    assert_eq!(
        store
            .verify_operator_token(&issued.token)
            .await
            .unwrap()
            .role,
        "viewer"
    );

    let listed = store.list_operator_credentials().await.unwrap();
    assert_eq!(listed.len(), 1);
    let listed_json = serde_json::to_string(&listed).unwrap();
    assert!(!listed_json.contains(&issued.token));
    assert!(!listed_json.contains("token_hash"));

    store
        .revoke_operator_credential(issued.credential.id)
        .await
        .unwrap();
    assert!(store.verify_operator_token(&issued.token).await.is_err());

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
                "plaintext operator token leaked into {}",
                candidate.display()
            );
        }
        let _ = std::fs::remove_file(candidate);
    }
}

#[tokio::test]
async fn only_active_admin_credentials_unlock_remote_bootstrap_requirement() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    assert!(!store.has_active_admin_operator_credential().await.unwrap());

    let viewer = store
        .issue_operator_credential("viewer", "viewer", 600)
        .await
        .unwrap();
    assert!(!store.has_active_admin_operator_credential().await.unwrap());

    let admin = store
        .issue_operator_credential("admin", "admin", 600)
        .await
        .unwrap();
    assert!(store.has_active_admin_operator_credential().await.unwrap());

    store
        .revoke_operator_credential(admin.credential.id)
        .await
        .unwrap();
    assert!(!store.has_active_admin_operator_credential().await.unwrap());

    store
        .revoke_operator_credential(viewer.credential.id)
        .await
        .unwrap();
}
