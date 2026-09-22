use super::*;
use ac_core::*;
use std::{
    path::{Path as FsPath, PathBuf},
    process::Stdio,
};
use tokio::{fs, io::AsyncWriteExt, process::Command};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/launch-profiles", get(profile_list).post(profile_create))
        .route("/launch-profiles/{id}", get(profile_get))
        .route("/launch-attempts/enqueue", post(launch_enqueue))
        .route("/launch-attempts/{id}", get(launch_get))
        .route("/tasks/{id}/launch-attempts", get(task_launches))
}

async fn profile_list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.list_launch_profiles().await?)))
}

async fn profile_create(
    State(s): State<AppState>,
    Json(input): Json<RegisterLaunchProfile>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.register_launch_profile(input).await?)))
}

async fn profile_get(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.get_launch_profile(id).await?)))
}

async fn launch_enqueue(
    State(s): State<AppState>,
    Json(input): Json<EnqueueLaunch>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.enqueue_launch(input).await?)))
}

async fn launch_get(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.get_launch_attempt(id).await?)))
}

async fn task_launches(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.task_launch_attempts(id).await?)))
}

pub async fn worker_loop(store: Store) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        interval.tick().await;
        loop {
            match store.claim_launch_job().await {
                Ok(Some(job)) => {
                    let child_store = store.clone();
                    tokio::spawn(async move {
                        if let Err(err) =
                            execute_claimed_launch(child_store.clone(), job.clone()).await
                        {
                            tracing::error!(
                                launch_attempt_id = %job.attempt.id,
                                error = %err,
                                "executor launch job failed"
                            );
                            let _ = child_store
                                .finish_launch_attempt(
                                    job.attempt.id,
                                    None,
                                    None,
                                    Some(err.to_string()),
                                )
                                .await;
                        }
                    });
                }
                Ok(None) => break,
                Err(err) => {
                    tracing::error!(%err, "launch job claim failed");
                    break;
                }
            }
        }
    }
}

async fn execute_claimed_launch(store: Store, job: ClaimedLaunchJob) -> anyhow::Result<()> {
    let execution = match store.begin_launch_attempt(job.attempt.id).await {
        Ok(value) => value,
        Err(err) => {
            let _ = store
                .finish_launch_attempt(
                    job.attempt.id,
                    None,
                    None,
                    Some(format!("begin launch failed: {err}")),
                )
                .await;
            return Err(anyhow::anyhow!(err));
        }
    };
    match execution.profile.adapter.as_str() {
        "codex_cli" => execute_codex(store, execution).await,
        other => {
            let message = format!("unsupported launch adapter at runtime: {other}");
            store
                .finish_launch_attempt(execution.attempt.id, None, None, Some(message.clone()))
                .await?;
            Err(anyhow::anyhow!(message))
        }
    }
}

async fn execute_codex(store: Store, execution: LaunchExecution) -> anyhow::Result<()> {
    let root = launch_root()?;
    execute_codex_with_root(store, execution, root).await
}

async fn execute_codex_with_root(
    store: Store,
    execution: LaunchExecution,
    root: PathBuf,
) -> anyhow::Result<()> {
    let cwd = execution
        .attempt
        .cwd
        .clone()
        .or_else(|| execution.profile.default_cwd.clone())
        .ok_or_else(|| anyhow::anyhow!("launch cwd missing"))?;
    if !FsPath::new(&cwd).is_dir() {
        anyhow::bail!("launch cwd is not a directory: {cwd}");
    }
    if !FsPath::new(&execution.profile.program).is_file() {
        anyhow::bail!(
            "launch program does not exist or is not a file: {}",
            execution.profile.program
        );
    }
    if let Some(home) = execution.profile.codex_home.as_deref()
        && !FsPath::new(home).is_dir()
    {
        anyhow::bail!("CODEX_HOME does not exist or is not a directory: {home}");
    }

    let dir = root.join(execution.attempt.id.to_string());
    fs::create_dir_all(&dir).await?;
    let stdout_path = dir.join("stdout.jsonl");
    let stderr_path = dir.join("stderr.log");
    let last_message_path = dir.join("last-message.txt");
    let stdout_file = std::fs::File::create(&stdout_path)?;
    let stderr_file = std::fs::File::create(&stderr_path)?;

    let args = codex_args(
        &execution.profile,
        &cwd,
        last_message_path.to_string_lossy().as_ref(),
    );
    let mut command = Command::new(&execution.profile.program);
    command
        .args(&args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .kill_on_drop(false);
    if let Some(home) = execution.profile.codex_home.as_deref() {
        command.env("CODEX_HOME", home);
    }
    let mut child = command.spawn()?;
    let pid = child.id().map(i64::from);
    store
        .mark_launch_running(
            execution.attempt.id,
            pid,
            stdout_path.to_string_lossy().into_owned(),
            stderr_path.to_string_lossy().into_owned(),
        )
        .await?;

    let prompt = build_prompt(&execution);
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(err) = stdin.write_all(prompt.as_bytes()).await {
            let _ = child.kill().await;
            let message = format!("failed writing launcher prompt to stdin: {err}");
            store
                .finish_launch_attempt(execution.attempt.id, None, None, Some(message.clone()))
                .await?;
            anyhow::bail!(message);
        }
    }

    let status = match child.wait().await {
        Ok(value) => value,
        Err(err) => {
            let message = format!("failed waiting for launcher process: {err}");
            store
                .finish_launch_attempt(execution.attempt.id, None, None, Some(message.clone()))
                .await?;
            anyhow::bail!(message);
        }
    };
    let exit_code = status.code().map(i64::from);
    let stdout = fs::read_to_string(&stdout_path).await.unwrap_or_default();
    let session_ref = extract_external_session_ref(&stdout);
    let error = if status.success() {
        None
    } else if let Some(message) = extract_codex_error(&stdout) {
        Some(format!("codex: {message}"))
    } else {
        Some(format!(
            "launcher process exited with {}",
            exit_code
                .map(|value| value.to_string())
                .unwrap_or_else(|| "signal".into())
        ))
    };
    store
        .finish_launch_attempt(execution.attempt.id, exit_code, session_ref, error)
        .await?;
    Ok(())
}

fn launch_root() -> anyhow::Result<PathBuf> {
    let configured = std::env::var("AC_LAUNCH_DIR").unwrap_or_else(|_| "data/launches".into());
    let path = PathBuf::from(configured);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn codex_args(profile: &LaunchProfile, cwd: &str, last_message_path: &str) -> Vec<String> {
    let mut args = vec![
        "exec".into(),
        "--json".into(),
        "--color".into(),
        "never".into(),
        "--approve-for-me".into(),
        "-C".into(),
        cwd.into(),
        "-o".into(),
        last_message_path.into(),
    ];
    if let Some(model) = profile.model.as_deref() {
        args.push("-m".into());
        args.push(model.into());
    }
    args.push("-".into());
    args
}

fn build_prompt(execution: &LaunchExecution) -> String {
    let context = execution.context.as_ref();
    let constraints = context
        .map(|value| clip(&value.constraints.to_string(), 6000))
        .unwrap_or_default();
    format!(
        "You are an executor launched by Agent Company.\n\
AgentInstance: {agent}\n\
Task ID: {task_id}\n\
Assignment ID: {assignment_id}\n\
Run ID: {run_id}\n\
\nUse the configured agent-company MCP server as the durable source of truth. The Assignment and Run already exist; do not claim the task or start another Run. Before substantial work, read task_get and context_get for Task ID {task_id}. Checkpoint meaningful progress to Run ID {run_id}. If the task is fully complete, call run_complete for Run ID {run_id}. If blocked or incomplete, checkpoint the blocker/progress and exit without calling run_complete.\n\
\nTask title:\n{title}\n\
\nTask description:\n{description}\n\
\nContext goal:\n{goal}\n\
\nContext summary:\n{summary}\n\
\nContext constraints (JSON, bounded):\n{constraints}\n",
        agent = execution.attempt.agent_instance_id,
        task_id = execution.task.id,
        assignment_id = execution.attempt.assignment_id,
        run_id = execution.run.id,
        title = clip(&execution.task.title, 2000),
        description = clip(&execution.task.description, 6000),
        goal = context.map(|v| clip(&v.goal, 4000)).unwrap_or_default(),
        summary = context
            .map(|v| clip(&v.current_summary, 6000))
            .unwrap_or_default(),
    )
}

fn clip(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn extract_external_session_ref(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(found) = find_session_ref(&value) {
            return Some(found);
        }
    }
    None
}

fn find_session_ref(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in ["thread_id", "session_id", "conversation_id"] {
                if let Some(Value::String(value)) = map.get(key)
                    && !value.trim().is_empty()
                {
                    return Some(value.clone());
                }
            }
            map.values().find_map(find_session_ref)
        }
        Value::Array(items) => items.iter().find_map(find_session_ref),
        _ => None,
    }
}

fn extract_codex_error(stdout: &str) -> Option<String> {
    for line in stdout.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(message) = find_error_message(&value) {
            return Some(message);
        }
    }
    None
}

fn find_error_message(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(kind)) = map.get("type")
                && kind == "error"
                && let Some(Value::String(message)) = map.get("message")
                && !message.trim().is_empty()
            {
                return Some(message.clone());
            }
            if let Some(Value::Object(error)) = map.get("error")
                && let Some(Value::String(message)) = error.get("message")
                && !message.trim().is_empty()
            {
                return Some(message.clone());
            }
            map.values().find_map(find_error_message)
        }
        Value::Array(items) => items.iter().find_map(find_error_message),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use std::os::unix::fs::PermissionsExt;
    use tower::ServiceExt;

    fn input<T: serde::de::DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    async fn make_agent(store: &Store) -> AgentInstance {
        let profile = store
            .register_profile(input(json!({
                "name":"launch-runtime-profile","provider":"local",
                "default_capabilities":["launch-test"]
            })))
            .await
            .unwrap();
        store
            .register_agent_instance(input(json!({
                "name":"launch-runtime-agent","profile_id":profile.id,
                "capabilities":["launch-test"]
            })))
            .await
            .unwrap()
    }

    async fn make_execution(
        store: &Store,
        program: &FsPath,
        cwd: &FsPath,
    ) -> (LaunchAttempt, AgentInstance) {
        let agent = make_agent(store).await;
        let task = store
            .create_task(input(json!({
                "title":"Unsafe-looking title ; rm -rf / && echo nope",
                "description":"$(touch /tmp/should-not-execute) is plain task text"
            })))
            .await
            .unwrap();
        store
            .create_context_revision(
                task.id,
                input(json!({
                    "goal":"prove task text goes through stdin only",
                    "current_summary":"fake launcher integration"
                })),
            )
            .await
            .unwrap();
        let assignment = store
            .claim_task(task.id, agent.id, "executor", 600)
            .await
            .unwrap();
        let profile = store
            .register_launch_profile(input(json!({
                "name":"fake codex",
                "adapter":"codex_cli",
                "agent_instance_id":agent.id,
                "program":program,
                "default_cwd":cwd,
                "enabled":true
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
        (attempt, agent)
    }

    fn fake_executable(exit_code: i32) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ac-launch-runtime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fake-codex");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\ncat >/dev/null\necho '{{\"type\":\"thread.started\",\"thread_id\":\"fake-session-123\"}}'\nexit {exit_code}\n"
            ),
        )
        .unwrap();
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
        path
    }

    async fn request(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn rest_launch_profile_enqueue_get_and_task_history() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = make_agent(&store).await;
        let task = store
            .create_task(input(json!({"title":"REST launch"})))
            .await
            .unwrap();
        let assignment = store
            .claim_task(task.id, agent.id, "executor", 600)
            .await
            .unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });
        let (status, profile) = request(
            &app,
            "POST",
            "/launch-profiles",
            json!({
                "name":"rest fake codex",
                "adapter":"codex_cli",
                "agent_instance_id":agent.id,
                "program":"/bin/echo",
                "default_cwd":std::env::temp_dir(),
                "enabled":true
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{profile}");
        let profile_id = profile["id"].as_str().unwrap();
        assert_eq!(
            request(&app, "GET", "/launch-profiles", json!(null))
                .await
                .1[0]["id"],
            profile["id"]
        );

        let (status, attempt) = request(
            &app,
            "POST",
            "/launch-attempts/enqueue",
            json!({
                "assignment_id":assignment.id,
                "launch_profile_id":profile_id
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{attempt}");
        assert_eq!(attempt["status"], "queued");
        let attempt_id = attempt["id"].as_str().unwrap();
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("/launch-attempts/{attempt_id}"),
                json!(null)
            )
            .await
            .1["assignment_id"],
            json!(assignment.id)
        );
        let history = request(
            &app,
            "GET",
            &format!("/tasks/{}/launch-attempts", task.id),
            json!(null),
        )
        .await
        .1;
        assert_eq!(history.as_array().unwrap().len(), 1);
    }

    #[test]
    fn codex_argv_never_contains_task_text() {
        let profile = LaunchProfile {
            id: uuid::Uuid::new_v4(),
            name: "codex".into(),
            adapter: "codex_cli".into(),
            agent_instance_id: uuid::Uuid::new_v4(),
            program: "/bin/codex".into(),
            codex_home: Some("/tmp/home".into()),
            default_cwd: Some("/tmp/work".into()),
            model: Some("gpt-test".into()),
            enabled: true,
            metadata: json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let args = codex_args(&profile, "/tmp/work", "/tmp/last");
        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert!(!args.join(" ").contains("task"));
        assert!(!args.iter().any(|arg| arg.contains("rm -rf")));
    }

    #[test]
    fn extracts_codex_thread_id_from_jsonl() {
        assert_eq!(
            extract_external_session_ref(
                "{\"type\":\"thread.started\",\"thread_id\":\"abc-123\"}\n{\"type\":\"item\"}"
            )
            .as_deref(),
            Some("abc-123")
        );
    }

    #[test]
    fn extracts_codex_failure_message_from_jsonl() {
        assert_eq!(
            extract_codex_error(
                "{\"type\":\"error\",\"message\":\"usage limited\"}\n{\"type\":\"turn.failed\",\"error\":{\"message\":\"final failure\"}}"
            )
            .as_deref(),
            Some("final failure")
        );
    }

    #[tokio::test]
    async fn fake_process_runs_through_launcher_and_requeues_without_run_complete() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let program = fake_executable(0);
        let cwd = program.parent().unwrap().to_path_buf();
        let (attempt, _) = make_execution(&store, &program, &cwd).await;
        let job = store.claim_launch_job().await.unwrap().unwrap();
        let execution = store.begin_launch_attempt(job.attempt.id).await.unwrap();
        execute_codex_with_root(store.clone(), execution, cwd.join("launches"))
            .await
            .unwrap();
        let finished = store.get_launch_attempt(attempt.id).await.unwrap();
        assert_eq!(finished.status, "completed");
        assert_eq!(
            finished.external_session_ref.as_deref(),
            Some("fake-session-123")
        );
        let run = store.get_run(finished.run_id.unwrap()).await.unwrap();
        assert_eq!(run.status, "paused");
        assert_eq!(
            store
                .get_assignment(finished.assignment_id)
                .await
                .unwrap()
                .status,
            "released"
        );
        assert_eq!(
            store.get_task(finished.task_id).await.unwrap().state,
            TaskState::Ready
        );
        let _ = std::fs::remove_dir_all(cwd);
    }

    #[tokio::test]
    async fn fake_nonzero_process_fails_run_and_releases_assignment() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let program = fake_executable(7);
        let cwd = program.parent().unwrap().to_path_buf();
        let (attempt, _) = make_execution(&store, &program, &cwd).await;
        let job = store.claim_launch_job().await.unwrap().unwrap();
        let execution = store.begin_launch_attempt(job.attempt.id).await.unwrap();
        execute_codex_with_root(store.clone(), execution, cwd.join("launches"))
            .await
            .unwrap();
        let finished = store.get_launch_attempt(attempt.id).await.unwrap();
        assert_eq!(finished.status, "failed");
        assert_eq!(finished.exit_code, Some(7));
        let run = store.get_run(finished.run_id.unwrap()).await.unwrap();
        assert_eq!(run.status, "failed");
        assert_eq!(
            store
                .get_assignment(finished.assignment_id)
                .await
                .unwrap()
                .status,
            "released"
        );
        let _ = std::fs::remove_dir_all(cwd);
    }
}
