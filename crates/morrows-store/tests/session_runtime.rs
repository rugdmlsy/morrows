use morrows_core::{CreateSession, RegisterLaunchProfile, StartSessionRuntime, UpdateSessionScope};
use morrows_store::Store;
use serde_json::json;

#[tokio::test]
async fn restart_recovery_fails_session_runtime_revokes_credential_and_requeues_delivery() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store
        .register_agent("session-runtime-recovery", &[])
        .await
        .unwrap();
    let session = store
        .create_session(CreateSession {
            agent_instance_id: agent.id,
            title: "Recovery".into(),
        })
        .await
        .unwrap();
    let message = store
        .create_human_session_message(session.id, "recover me")
        .await
        .unwrap();

    let temp = std::env::temp_dir();
    let profile = store
        .register_launch_profile(
            serde_json::from_value::<RegisterLaunchProfile>(json!({
                "name": "session runtime recovery",
                "adapter": "codex_cli",
                "agent_instance_id": agent.id,
                "program": "/bin/echo",
                "default_cwd": temp,
                "enabled": true
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let runtime = store
        .create_session_runtime_attempt(
            session.id,
            StartSessionRuntime {
                launch_profile_id: Some(profile.id),
                model: None,
                reasoning_effort: None,
            },
        )
        .await
        .unwrap();
    store
        .mark_session_runtime_running(
            runtime.id,
            Some(123),
            "/tmp/session-runtime-out".into(),
            "/tmp/session-runtime-err".into(),
        )
        .await
        .unwrap();
    let credential = store
        .issue_session_runtime_credential(agent.id, session.id, "recovery test", 600)
        .await
        .unwrap();
    let claimed = store
        .claim_session_deliveries_for_runtime(runtime.id, 10)
        .await
        .unwrap();
    assert!(
        claimed
            .iter()
            .any(|delivery| delivery.source_id == message.id)
    );

    assert_eq!(
        store
            .recover_session_runtime_attempts_after_restart()
            .await
            .unwrap(),
        1
    );
    assert_eq!(store.recover_agent_delivery_claims().await.unwrap(), 1);

    let recovered = store.get_session_runtime_attempt(runtime.id).await.unwrap();
    assert_eq!(recovered.status, "failed");
    assert_eq!(recovered.error.as_deref(), Some("morrows_server_restarted"));

    let credential = store
        .get_agent_credential(credential.credential.id)
        .await
        .unwrap();
    assert!(credential.revoked_at.is_some());

    let inbox = store.agent_delivery_inbox(agent.id, 10).await.unwrap();
    assert_eq!(inbox.len(), 1);
    assert_eq!(inbox[0].source_id, message.id);
    assert_eq!(inbox[0].status, "queued");
}

#[tokio::test]
async fn task_launch_and_direct_runtime_are_mutually_exclusive_for_one_session() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store
        .register_agent("session-runtime-exclusion", &[])
        .await
        .unwrap();
    let task = store
        .create_task(
            serde_json::from_value(json!({
                "title": "Scoped execution",
                "description": "One Provider thread at a time"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let assignment = store
        .claim_task(task.id, agent.id, "executor", 600)
        .await
        .unwrap();
    let session = store
        .create_scoped_session(
            CreateSession {
                agent_instance_id: agent.id,
                title: "Scoped execution".into(),
            },
            None,
            Some(task.id),
        )
        .await
        .unwrap();
    let profile = store
        .register_launch_profile(
            serde_json::from_value::<RegisterLaunchProfile>(json!({
                "name": "session runtime exclusion",
                "adapter": "codex_cli",
                "agent_instance_id": agent.id,
                "program": "/bin/echo",
                "default_cwd": std::env::temp_dir(),
                "enabled": true
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let direct = store
        .create_session_runtime_attempt(
            session.id,
            StartSessionRuntime {
                launch_profile_id: Some(profile.id),
                model: None,
                reasoning_effort: None,
            },
        )
        .await
        .unwrap();

    let blocked_launch = store
        .enqueue_launch(
            serde_json::from_value(json!({
                "assignment_id": assignment.id,
                "launch_profile_id": profile.id
            }))
            .unwrap(),
        )
        .await;
    assert!(matches!(
        blocked_launch,
        Err(morrows_core::DomainError::Conflict(_))
    ));

    store
        .finish_session_runtime_attempt(direct.id, Some(0), None, None)
        .await
        .unwrap();
    let task_launch = store
        .enqueue_launch(
            serde_json::from_value(json!({
                "assignment_id": assignment.id,
                "launch_profile_id": profile.id
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(task_launch.session_id, Some(session.id));

    let blocked_direct = store
        .create_session_runtime_attempt(
            session.id,
            StartSessionRuntime {
                launch_profile_id: Some(profile.id),
                model: None,
                reasoning_effort: None,
            },
        )
        .await;
    assert!(matches!(
        blocked_direct,
        Err(morrows_core::DomainError::Conflict(_))
    ));

    let blocked_scope_change = store
        .update_session_scope(
            session.id,
            UpdateSessionScope {
                project_id: None,
                task_id: None,
            },
        )
        .await;
    assert!(matches!(
        blocked_scope_change,
        Err(morrows_core::DomainError::Conflict(_))
    ));
}

#[tokio::test]
async fn next_session_runtime_inherits_model_and_reasoning_effort() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store
        .register_agent("session-runtime-inherit", &[])
        .await
        .unwrap();
    let session = store
        .create_session(CreateSession {
            agent_instance_id: agent.id,
            title: "Inheritance".into(),
        })
        .await
        .unwrap();
    let profile = store
        .register_launch_profile(
            serde_json::from_value::<RegisterLaunchProfile>(json!({
                "name": "session runtime inherit",
                "adapter": "codex_cli",
                "agent_instance_id": agent.id,
                "program": "/bin/echo",
                "default_cwd": std::env::temp_dir(),
                "enabled": true
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let first = store
        .create_session_runtime_attempt(
            session.id,
            StartSessionRuntime {
                launch_profile_id: Some(profile.id),
                model: Some("gpt-5.6-sol".into()),
                reasoning_effort: Some("high".into()),
            },
        )
        .await
        .unwrap();
    store
        .finish_session_runtime_attempt(first.id, Some(1), None, Some("test".into()))
        .await
        .unwrap();

    let second = store
        .create_session_runtime_attempt(
            session.id,
            StartSessionRuntime {
                launch_profile_id: Some(profile.id),
                model: None,
                reasoning_effort: None,
            },
        )
        .await
        .unwrap();

    assert_eq!(second.model.as_deref(), Some("gpt-5.6-sol"));
    assert_eq!(second.reasoning_effort.as_deref(), Some("high"));
}
