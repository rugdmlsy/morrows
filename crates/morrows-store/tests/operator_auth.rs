use morrows_store::Store;

#[tokio::test]
async fn browser_login_requires_local_approval_and_preserves_revocation() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let login = store.request_operator_login("test browser").await.unwrap();
    assert!(store.verify_operator_token(&login.token).await.is_err());
    assert_eq!(
        store
            .operator_login_status(login.id, &login.token)
            .await
            .unwrap()["status"],
        "pending"
    );
    assert!(
        store
            .operator_login_status(login.id, "wrong")
            .await
            .is_err()
    );
    assert!(
        store
            .approve_operator_login("wrong", "operator", 600)
            .await
            .is_err()
    );
    let (first, retry) = tokio::join!(
        store.approve_operator_login(&login.code, "operator", 600),
        store.approve_operator_login(&login.code, "operator", 600)
    );
    let first = first.unwrap();
    assert_eq!(first.id, retry.unwrap().id);
    let elevated_retry = store
        .approve_operator_login(&login.code, "admin", 10000)
        .await
        .unwrap();
    assert_eq!(elevated_retry.role, "operator");
    assert_eq!(elevated_retry.expires_at, first.expires_at);
    assert_eq!(store.list_operator_credentials().await.unwrap().len(), 1);
    assert_eq!(
        store.verify_operator_token(&login.token).await.unwrap().id,
        first.id
    );
    assert_eq!(
        store
            .operator_login_status(login.id, &login.token)
            .await
            .unwrap()["status"],
        "approved"
    );
    store.revoke_operator_credential(first.id).await.unwrap();
    assert!(store.verify_operator_token(&login.token).await.is_err());
    assert_eq!(
        store
            .operator_login_status(login.id, &login.token)
            .await
            .unwrap()["status"],
        "expired"
    );
}

#[tokio::test]
async fn browser_login_expiry_and_secret_storage() {
    let path = std::env::temp_dir().join(format!("morrows-login-{}.db", uuid::Uuid::new_v4()));
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let store = Store::connect(&url).await.unwrap();
    let login = store.request_operator_login("expiry").await.unwrap();
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    sqlx::query("UPDATE operator_login_requests SET expires_at='2000-01-01T00:00:00Z' WHERE id=?")
        .bind(login.id.to_string())
        .execute(&pool)
        .await
        .unwrap();
    assert!(
        store
            .approve_operator_login(&login.code, "operator", 600)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .operator_login_status(login.id, &login.token)
            .await
            .unwrap()["status"],
        "expired"
    );
    assert!(store.list_operator_credentials().await.unwrap().is_empty());
    pool.close().await;
    drop(store);
    for file in [
        path.clone(),
        path.with_extension("db-wal"),
        path.with_extension("db-shm"),
    ] {
        if let Ok(bytes) = std::fs::read(&file) {
            assert!(
                !bytes
                    .windows(login.token.len())
                    .any(|window| window == login.token.as_bytes())
            );
        }
        let _ = std::fs::remove_file(file);
    }
}

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
