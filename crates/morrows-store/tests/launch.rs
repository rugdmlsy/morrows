use morrows_core::*;
use morrows_store::Store;
use serde_json::{Value, json};
use std::path::PathBuf;

fn input<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

async fn agent(store: &Store, name: &str) -> AgentInstance {
    let profile = store
        .register_profile(input(json!({
            "name":format!("{name}-profile"),
            "provider":"local",
            "default_capabilities":["code"]
        })))
        .await
        .unwrap();
    store
        .register_agent_instance(input(json!({
            "name":name,
            "profile_id":profile.id,
            "capabilities":["code"]
        })))
        .await
        .unwrap()
}

async fn assignment(store: &Store, agent: Id, title: &str) -> Assignment {
    let task = store
        .create_task(input(json!({"title":title})))
        .await
        .unwrap();
    store
        .claim_task(task.id, agent, "executor", 600)
        .await
        .unwrap()
}

fn tmp_dir() -> String {
    std::env::temp_dir().to_string_lossy().into_owned()
}

fn profile_input(agent: Id) -> RegisterLaunchProfile {
    input(json!({
        "name":"test launcher",
        "adapter":"codex_cli",
        "agent_instance_id":agent,
        "program":"/bin/echo",
        "default_cwd":tmp_dir(),
        "enabled":true,
        "metadata":{"test":true}
    }))
}

#[tokio::test]
async fn launch_profile_validation_and_enqueue_are_safe_and_idempotent() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "worker").await;

    let mut relative = profile_input(worker.id);
    relative.program = "codex".into();
    assert!(matches!(
        store.register_launch_profile(relative).await,
        Err(DomainError::InvalidInput(_))
    ));

    let mut relative_cwd = profile_input(worker.id);
    relative_cwd.default_cwd = Some("relative".into());
    assert!(matches!(
        store.register_launch_profile(relative_cwd).await,
        Err(DomainError::InvalidInput(_))
    ));

    let profile = store
        .register_launch_profile(profile_input(worker.id))
        .await
        .unwrap();
    let first_assignment = assignment(&store, worker.id, "launch").await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":first_assignment.id,
            "launch_profile_id":profile.id
        })))
        .await
        .unwrap();
    assert_eq!(attempt.status, "queued");
    assert_eq!(attempt.agent_instance_id, worker.id);
    assert!(PathBuf::from(attempt.cwd.unwrap()).is_absolute());

    let second_assignment = assignment(&store, worker.id, "cwd override").await;
    assert!(matches!(
        store
            .enqueue_launch(input(json!({
                "assignment_id":second_assignment.id,
                "launch_profile_id":profile.id,
                "cwd":"/"
            })))
            .await,
        Err(DomainError::Conflict(_))
    ));

    assert!(matches!(
        store
            .enqueue_launch(input(json!({
                "assignment_id":first_assignment.id,
                "launch_profile_id":profile.id
            })))
            .await,
        Err(DomainError::Conflict(_))
    ));

    let a = store.clone();
    let b = store.clone();
    let (first, second) = tokio::join!(a.claim_launch_job(), b.claim_launch_job());
    let claimed = [first.unwrap(), second.unwrap()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(claimed.len(), 1);
    assert_eq!(claimed[0].attempt.id, attempt.id);
    assert!(store.claim_launch_job().await.unwrap().is_none());
}

#[tokio::test]
async fn clean_process_exit_without_semantic_completion_pauses_run_and_requeues_task() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "worker").await;
    let profile = store
        .register_launch_profile(profile_input(worker.id))
        .await
        .unwrap();
    let assignment = assignment(&store, worker.id, "exit zero").await;
    let task_id = assignment.task_id;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment.id,
            "launch_profile_id":profile.id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
    store
        .mark_launch_running(
            attempt.id,
            Some(123),
            "/tmp/out.jsonl".into(),
            "/tmp/err.log".into(),
        )
        .await
        .unwrap();
    let done = store
        .finish_launch_attempt(attempt.id, Some(0), Some("session-123".into()), None)
        .await
        .unwrap();
    assert_eq!(done.status, "completed");
    assert_eq!(done.external_session_ref.as_deref(), Some("session-123"));
    let run = store.get_run(execution.run.id).await.unwrap();
    assert_eq!(run.status, "paused");
    assert_eq!(
        run.stop_reason.as_deref(),
        Some("launcher_process_exited_without_completion")
    );
    let assignment = store.get_assignment(assignment.id).await.unwrap();
    assert_eq!(assignment.status, "released");
    assert_eq!(
        store.get_task(task_id).await.unwrap().state,
        TaskState::Ready
    );
}

#[tokio::test]
async fn agent_semantic_completion_wins_over_launcher_process_exit() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "worker").await;
    let profile = store
        .register_launch_profile(profile_input(worker.id))
        .await
        .unwrap();
    let assignment = assignment(&store, worker.id, "semantic completion").await;
    let task_id = assignment.task_id;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment.id,
            "launch_profile_id":profile.id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
    store
        .mark_launch_running(
            attempt.id,
            Some(456),
            "/tmp/out.jsonl".into(),
            "/tmp/err.log".into(),
        )
        .await
        .unwrap();

    store
        .complete_run(
            execution.run.id,
            worker.id,
            json!({"summary":"agent completed semantically"}),
        )
        .await
        .unwrap();
    store
        .finish_launch_attempt(attempt.id, Some(0), Some("session-complete".into()), None)
        .await
        .unwrap();

    let run = store.get_run(execution.run.id).await.unwrap();
    assert_eq!(run.status, "completed");
    assert_eq!(
        run.external_session_ref.as_deref(),
        Some("session-complete")
    );
    assert_eq!(
        store.get_task(task_id).await.unwrap().state,
        TaskState::Done
    );
    assert_eq!(
        store.get_assignment(assignment.id).await.unwrap().status,
        "completed"
    );
}

#[tokio::test]
async fn failed_launch_releases_assignment_and_marks_run_failed() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "worker").await;
    let profile = store
        .register_launch_profile(profile_input(worker.id))
        .await
        .unwrap();
    let assignment = assignment(&store, worker.id, "failure").await;
    let task_id = assignment.task_id;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":assignment.id,
            "launch_profile_id":profile.id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
    let done = store
        .finish_launch_attempt(attempt.id, Some(7), None, Some("process exited 7".into()))
        .await
        .unwrap();
    assert_eq!(done.status, "failed");
    let run = store.get_run(execution.run.id).await.unwrap();
    assert_eq!(run.status, "failed");
    assert_eq!(
        store.get_assignment(assignment.id).await.unwrap().status,
        "released"
    );
    assert_eq!(
        store.get_task(task_id).await.unwrap().state,
        TaskState::Ready
    );
}

#[tokio::test]
async fn launch_profile_must_match_assignment_agent_and_executor_role() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = agent(&store, "a").await;
    let b = agent(&store, "b").await;
    let b_profile = store
        .register_launch_profile(profile_input(b.id))
        .await
        .unwrap();
    let executor = assignment(&store, a.id, "mismatch").await;
    assert!(matches!(
        store
            .enqueue_launch(input(json!({
                "assignment_id":executor.id,
                "launch_profile_id":b_profile.id
            })))
            .await,
        Err(DomainError::Conflict(_))
    ));

    let task = store
        .create_task(input(json!({"title":"review launch rejected"})))
        .await
        .unwrap();
    let review = store
        .claim_task(task.id, a.id, "reviewer", 600)
        .await
        .unwrap();
    let a_profile = store
        .register_launch_profile(profile_input(a.id))
        .await
        .unwrap();
    assert!(matches!(
        store
            .enqueue_launch(input(json!({
                "assignment_id":review.id,
                "launch_profile_id":a_profile.id
            })))
            .await,
        Err(DomainError::Conflict(_))
    ));
}

#[tokio::test]
async fn external_agent_accepts_only_its_own_assignment_and_completes_durably() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "lsm-worker").await;
    let other = agent(&store, "other-worker").await;
    let profile = store
        .register_launch_profile(input(json!({
            "name":"LSM external", "adapter":"lsm_external", "agent_instance_id":worker.id
        })))
        .await
        .unwrap();
    let work = assignment(&store, worker.id, "external handoff").await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":work.id, "launch_profile_id":profile.id
        })))
        .await
        .unwrap();
    assert_eq!(attempt.status, "awaiting_agent");
    assert!(attempt.job_id.is_none());
    assert!(store.claim_launch_job().await.unwrap().is_none());
    assert_eq!(
        store
            .pending_external_launches(worker.id)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        store
            .accept_external_launch(
                input(json!({
                    "launch_attempt_id":attempt.id, "external_session_ref":"lsm-session"
                })),
                other.id
            )
            .await,
        Err(DomainError::Conflict(_))
    ));
    let instruction = store
        .send_launch_instruction(
            input(json!({
                "launch_attempt_id":attempt.id, "body":"Review the task context"
            })),
            None,
        )
        .await
        .unwrap();
    assert_eq!(instruction.body, "Review the task context");
    assert_eq!(
        store.launch_instructions(attempt.id).await.unwrap().len(),
        1
    );
    let accepted = store
        .accept_external_launch(
            input(json!({
                "launch_attempt_id":attempt.id, "external_session_ref":"lsm-session"
            })),
            worker.id,
        )
        .await
        .unwrap();
    assert_eq!(accepted.status, "running");
    let run_id = accepted.run_id.unwrap();
    assert!(
        store
            .accept_external_launch(
                input(json!({
                    "launch_attempt_id":attempt.id, "external_session_ref":"second-session"
                })),
                worker.id
            )
            .await
            .is_err()
    );
    store
        .complete_run(run_id, worker.id, json!({"summary":"done"}))
        .await
        .unwrap();
    assert_eq!(store.reconcile_external_launches().await.unwrap(), 1);
    assert_eq!(
        store.get_launch_attempt(attempt.id).await.unwrap().status,
        "completed"
    );
    assert_eq!(
        store.get_task(work.task_id).await.unwrap().state,
        TaskState::Done
    );
}

#[tokio::test]
async fn codex_resume_is_scoped_and_stop_revokes_work() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "resume-worker").await;
    let profile = store
        .register_launch_profile(profile_input(worker.id))
        .await
        .unwrap();
    let work = assignment(&store, worker.id, "resume task").await;
    let first = store
        .enqueue_launch(input(json!({
            "assignment_id":work.id, "launch_profile_id":profile.id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    store.begin_launch_attempt(first.id).await.unwrap();
    store
        .finish_launch_attempt(first.id, Some(0), Some("session-1".into()), None)
        .await
        .unwrap();
    let new_assignment = store
        .claim_task(work.task_id, worker.id, "executor", 600)
        .await
        .unwrap();
    let resumed = store
        .enqueue_launch(input(json!({
            "assignment_id":new_assignment.id,
            "launch_profile_id":profile.id,
            "resume_from_attempt_id":first.id
        })))
        .await
        .unwrap();
    assert_eq!(resumed.resume_from_attempt_id, Some(first.id));
    assert_eq!(
        store.stop_launch_attempt(resumed.id).await.unwrap().status,
        "cancelled"
    );
    assert!(store.claim_launch_job().await.unwrap().is_none());
    assert_eq!(
        store
            .get_assignment(new_assignment.id)
            .await
            .unwrap()
            .status,
        "released"
    );
    assert_eq!(
        store.get_task(work.task_id).await.unwrap().state,
        TaskState::Ready
    );
}

#[tokio::test]
async fn restart_reconciles_unfinished_local_launcher() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "restart-worker").await;
    let profile = store
        .register_launch_profile(profile_input(worker.id))
        .await
        .unwrap();
    let work = assignment(&store, worker.id, "interrupted launch").await;
    let attempt = store
        .enqueue_launch(input(json!({
            "assignment_id":work.id, "launch_profile_id":profile.id
        })))
        .await
        .unwrap();
    store.claim_launch_job().await.unwrap().unwrap();
    store.begin_launch_attempt(attempt.id).await.unwrap();
    assert_eq!(store.recover_launch_jobs_after_restart().await.unwrap(), 1);
    assert_eq!(
        store.get_launch_attempt(attempt.id).await.unwrap().status,
        "failed"
    );
    assert_eq!(
        store.get_assignment(work.id).await.unwrap().status,
        "released"
    );
}
