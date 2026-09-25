use super::*;
use axum::extract::Query;
use morrows_core::{
    CreateSession, CreateSessionSummaryRevision, SessionRuntimeAttempt, SessionSummaryRevision,
    StartSessionRuntime, UpdateSessionScope,
};
use std::{path::PathBuf, process::Stdio};
use tokio::{fs, io::AsyncWriteExt, process::Command};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sessions", get(list).post(create))
        .route("/sessions/{id}", get(get_one))
        .route("/sessions/{id}/scope", post(scope_update))
        .route(
            "/sessions/{id}/messages",
            get(messages).post(message_create),
        )
        .route(
            "/sessions/{id}/messages/{message_id}/recall",
            post(message_recall),
        )
        .route(
            "/sessions/{id}/summary",
            get(summary_get).post(summary_create),
        )
        .route("/sessions/{id}/summary/history", get(summary_history))
        .route("/sessions/{id}/runtime", get(runtime_get))
        .route("/sessions/{id}/runtime/options", get(runtime_options))
        .route("/sessions/{id}/runtime/start", post(runtime_start))
}

#[derive(Debug, Deserialize)]
struct SessionListQuery {
    agent_instance_id: Option<Id>,
    project_id: Option<Id>,
    task_id: Option<Id>,
}

async fn list(
    State(state): State<AppState>,
    Query(query): Query<SessionListQuery>,
) -> Result<Json<Value>, ApiError> {
    let mut sessions = state.store.list_sessions(query.agent_instance_id).await?;
    if let Some(project_id) = query.project_id {
        sessions.retain(|session| session.project_id == Some(project_id));
    }
    if let Some(task_id) = query.task_id {
        sessions.retain(|session| session.task_id == Some(task_id));
    }
    Ok(Json(json!(sessions)))
}

#[derive(Debug, Deserialize)]
struct CreateSessionRequest {
    agent_instance_id: Id,
    #[serde(default)]
    title: String,
    #[serde(default)]
    project_id: Option<Id>,
    #[serde(default)]
    task_id: Option<Id>,
}

async fn create(
    State(state): State<AppState>,
    Json(input): Json<CreateSessionRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .create_scoped_session(
                CreateSession {
                    agent_instance_id: input.agent_instance_id,
                    title: input.title,
                },
                input.project_id,
                input.task_id,
            )
            .await?
    )))
}

async fn scope_update(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<UpdateSessionScope>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state.store.update_session_scope(id, input).await?
    )))
}

async fn get_one(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(state.store.get_session(id).await?)))
}

#[derive(Debug, Deserialize)]
struct SessionMessagesQuery {
    before: Option<Id>,
    after: Option<Id>,
    #[serde(default = "default_message_limit")]
    limit: i64,
}

fn default_message_limit() -> i64 {
    80
}

async fn messages(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    Query(query): Query<SessionMessagesQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .session_history(id, query.before, query.after, query.limit)
            .await?
    )))
}

#[derive(Debug, Deserialize)]
struct CreateSessionMessageBody {
    body: String,
    #[serde(default)]
    client_message_id: Option<String>,
}

async fn message_create(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<CreateSessionMessageBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .create_human_session_message_idempotent(
                id,
                &input.body,
                input.client_message_id.as_deref(),
            )
            .await?
    )))
}

async fn message_recall(
    State(state): State<AppState>,
    Path((id, message_id)): Path<(Id, Id)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .recall_human_session_message(id, message_id)
            .await?
    )))
}

async fn runtime_get(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state.store.latest_session_runtime_attempt(id).await?
    )))
}

async fn runtime_options(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    let session = state.store.get_session(id).await?;
    let latest = state.store.latest_session_runtime_attempt(id).await?;
    let profiles = state
        .store
        .list_launch_profiles()
        .await?
        .into_iter()
        .filter(|profile| {
            profile.agent_instance_id == session.agent_instance_id
                && profile.enabled
                && matches!(profile.adapter.as_str(), "codex_cli" | "codebuddy_cli")
        })
        .collect::<Vec<_>>();

    let profile = latest
        .as_ref()
        .and_then(|attempt| {
            profiles
                .iter()
                .find(|profile| profile.id == attempt.launch_profile_id)
        })
        .or_else(|| (profiles.len() == 1).then(|| &profiles[0]));

    let Some(profile) = profile else {
        return Ok(Json(json!({
            "available": false,
            "reason": if profiles.is_empty() {
                "no enabled local CLI launch profile"
            } else {
                "multiple local CLI launch profiles; choose a profile first"
            },
            "models": [],
            "reasoning_efforts": [],
            "selected_model": "",
            "selected_reasoning_effort": ""
        })));
    };

    let (models, efforts) = discover_runtime_model_options(profile).await;
    Ok(Json(json!({
        "available": true,
        "adapter": profile.adapter,
        "launch_profile_id": profile.id,
        "models": models,
        "reasoning_efforts": efforts,
        "selected_model": latest.as_ref().and_then(|attempt| attempt.model.clone()).or_else(|| profile.model.clone()).unwrap_or_default(),
        "selected_reasoning_effort": latest.as_ref().and_then(|attempt| attempt.reasoning_effort.clone()).unwrap_or_default()
    })))
}

async fn discover_runtime_model_options(
    profile: &morrows_core::LaunchProfile,
) -> (Vec<Value>, Vec<String>) {
    match profile.adapter.as_str() {
        "codex_cli" => {
            let mut command = Command::new(&profile.program);
            command.args(["debug", "models", "--bundled"]);
            if let Some(home) = profile.codex_home.as_deref() {
                command.env("CODEX_HOME", home);
            }
            let Ok(output) = command.output().await else {
                return (Vec::new(), Vec::new());
            };
            if !output.status.success() {
                return (Vec::new(), Vec::new());
            }
            let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
                return (Vec::new(), Vec::new());
            };
            let mut global_efforts = Vec::<String>::new();
            let mut models = Vec::new();
            for model in value
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                if model.get("visibility").and_then(Value::as_str) != Some("list") {
                    continue;
                }
                let id = model
                    .get("slug")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if id.is_empty() {
                    continue;
                }
                let efforts = model
                    .get("supported_reasoning_levels")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item.get("effort").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                for effort in &efforts {
                    if !global_efforts.contains(effort) {
                        global_efforts.push(effort.clone());
                    }
                }
                models.push(json!({
                    "id": id,
                    "label": model.get("display_name").and_then(Value::as_str).unwrap_or(id),
                    "reasoning_efforts": efforts,
                    "default_reasoning_effort": model.get("default_reasoning_level").and_then(Value::as_str).unwrap_or("")
                }));
            }
            (models, global_efforts)
        }
        "codebuddy_cli" => {
            let Ok(output) = Command::new(&profile.program).arg("--help").output().await else {
                return (Vec::new(), Vec::new());
            };
            let help = format!(
                "{}
{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let models = extract_parenthesized_after(&help, "Currently supported: (")
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|id| json!({"id": id, "label": id, "reasoning_efforts": ["minimal","low","medium","high","xhigh","max"], "default_reasoning_effort": ""}))
                .collect::<Vec<_>>();
            let efforts = extract_parenthesized_after(&help, "Reasoning effort level (")
                .unwrap_or_else(|| "minimal, low, medium, high, xhigh, max".into())
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>();
            (models, efforts)
        }
        _ => (Vec::new(), Vec::new()),
    }
}

fn extract_parenthesized_after(input: &str, marker: &str) -> Option<String> {
    let start = input.find(marker)? + marker.len();
    let rest = &input[start..];
    let end = rest.find(')')?;
    Some(rest[..end].to_owned())
}

async fn runtime_start(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<StartSessionRuntime>,
) -> Result<Json<Value>, ApiError> {
    let attempt = state
        .store
        .create_session_runtime_attempt(id, input)
        .await?;
    let child_store = state.store.clone();
    let child_attempt = attempt.clone();
    tokio::spawn(async move {
        if let Err(err) = execute_session_runtime(child_store.clone(), child_attempt.clone()).await
        {
            tracing::error!(
                session_id = %child_attempt.session_id,
                runtime_attempt_id = %child_attempt.id,
                error = %err,
                "Session runtime failed"
            );
            let _ = child_store
                .release_session_runtime_deliveries(child_attempt.id)
                .await;
            let _ = child_store
                .revoke_session_agent_credentials(child_attempt.session_id)
                .await;
            let _ = child_store
                .finish_session_runtime_attempt(child_attempt.id, None, None, Some(err.to_string()))
                .await;
        }
    });
    Ok(Json(json!(attempt)))
}

async fn execute_session_runtime(
    store: Store,
    attempt: SessionRuntimeAttempt,
) -> anyhow::Result<()> {
    let profile = store.get_launch_profile(attempt.launch_profile_id).await?;
    store.get_session(attempt.session_id).await?;
    let account = match attempt.account_id {
        Some(id) => Some(store.get_account(id).await?),
        None => None,
    };
    let cwd = attempt
        .cwd
        .clone()
        .ok_or_else(|| anyhow::anyhow!("Session runtime cwd missing"))?;
    let previous_ref = store
        .latest_session_provider_ref(attempt.session_id, attempt.launch_profile_id)
        .await?;
    let credential = store
        .issue_session_runtime_credential(
            attempt.agent_instance_id,
            attempt.session_id,
            &format!("Session runtime {}", attempt.id),
            3600,
        )
        .await?;

    let root = session_runtime_root()?;
    let dir = root.join(attempt.id.to_string());
    fs::create_dir_all(&dir).await?;
    let stdout_path = dir.join("stdout.jsonl");
    let stderr_path = dir.join("stderr.log");
    let stdout_file = std::fs::File::create(&stdout_path)?;
    let stderr_file = std::fs::File::create(&stderr_path)?;

    let result = match profile.adapter.as_str() {
        "codex_cli" => {
            let last_message_path = dir.join("last-message.txt");
            let mut runtime_profile = profile.clone();
            runtime_profile.model = attempt.model.clone().or(profile.model.clone());
            let mut args = super::launch::codex_args(
                &runtime_profile,
                &cwd,
                last_message_path.to_string_lossy().as_ref(),
                previous_ref.as_deref(),
            );
            super::launch::inject_morrows_config(&mut args);
            if let Some(effort) = attempt.reasoning_effort.as_deref() {
                args.insert(1, format!("model_reasoning_effort=\"{effort}\""));
                args.insert(1, "-c".into());
            }
            let mut command = Command::new(&profile.program);
            command
                .args(&args)
                .current_dir(&cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::from(stdout_file))
                .stderr(Stdio::from(stderr_file))
                .kill_on_drop(true);
            if let Some(home) = profile.codex_home.as_deref() {
                command.env("CODEX_HOME", home);
            }
            configure_session_runtime_env(
                &mut command,
                &attempt,
                account.as_ref().map(|value| value.id),
                &credential.token,
            );
            run_session_runtime_child(&store, &attempt, command, &stdout_path, &stderr_path, None)
                .await
        }
        "codebuddy_cli" => {
            let mcp_path = dir.join("codebuddy-mcp.json");
            write_session_codebuddy_mcp(&mcp_path, attempt.agent_instance_id, &credential.token)
                .await?;
            let provider_ref = previous_ref
                .clone()
                .unwrap_or_else(|| attempt.id.to_string());
            let mut runtime_profile = profile.clone();
            runtime_profile.model = attempt.model.clone().or(profile.model.clone());
            let mut args = super::launch::codebuddy_args(
                &runtime_profile,
                mcp_path.to_string_lossy().as_ref(),
                &provider_ref,
                previous_ref.is_some(),
            );
            if let Some(effort) = attempt.reasoning_effort.as_deref() {
                args.extend(["--effort".into(), effort.into()]);
            }
            let mut command = Command::new(&profile.program);
            command
                .args(&args)
                .current_dir(&cwd)
                .stdin(Stdio::piped())
                .stdout(Stdio::from(stdout_file))
                .stderr(Stdio::from(stderr_file))
                .kill_on_drop(true);
            configure_session_runtime_env(
                &mut command,
                &attempt,
                account.as_ref().map(|value| value.id),
                &credential.token,
            );
            let result = run_session_runtime_child(
                &store,
                &attempt,
                command,
                &stdout_path,
                &stderr_path,
                Some(provider_ref),
            )
            .await;
            let _ = fs::remove_file(mcp_path).await;
            result
        }
        other => Err(anyhow::anyhow!(
            "Session runtime requires local CLI adapter, got {other}"
        )),
    };

    let _ = store
        .revoke_agent_credential(credential.credential.id)
        .await;
    result
}

fn configure_session_runtime_env(
    command: &mut Command,
    attempt: &SessionRuntimeAttempt,
    account_id: Option<Id>,
    token: &str,
) {
    command.env_remove("MORROWS_AGENT_AUTHORIZATION");
    command.env_remove("MORROWS_AGENT_INSTANCE_ID");
    command.env_remove("MORROWS_SESSION_ID");
    command.env_remove("MORROWS_ACCOUNT_ID");
    command.env("MORROWS_AGENT_AUTHORIZATION", format!("Bearer {token}"));
    command.env(
        "MORROWS_AGENT_INSTANCE_ID",
        attempt.agent_instance_id.to_string(),
    );
    command.env("MORROWS_SESSION_ID", attempt.session_id.to_string());
    if let Some(account_id) = account_id {
        command.env("MORROWS_ACCOUNT_ID", account_id.to_string());
    }
}

async fn run_session_runtime_child(
    store: &Store,
    attempt: &SessionRuntimeAttempt,
    mut command: Command,
    stdout_path: &std::path::Path,
    stderr_path: &std::path::Path,
    known_provider_ref: Option<String>,
) -> anyhow::Result<()> {
    let mut child = command.spawn()?;
    let pid = child.id().map(i64::from);
    store
        .mark_session_runtime_running(
            attempt.id,
            pid,
            stdout_path.to_string_lossy().into_owned(),
            stderr_path.to_string_lossy().into_owned(),
        )
        .await?;

    let session = store.get_session(attempt.session_id).await?;
    let summary = store
        .get_latest_session_summary_revision(attempt.session_id)
        .await?;
    let deliveries = store
        .claim_session_deliveries_for_runtime(attempt.id, 100)
        .await?;
    let prompt = build_session_runtime_prompt(attempt, &session, summary.as_ref(), &deliveries);

    let Some(mut stdin) = child.stdin.take() else {
        let _ = child.kill().await;
        store.release_session_runtime_deliveries(attempt.id).await?;
        return Err(anyhow::anyhow!("Session runtime stdin is unavailable"));
    };
    if let Err(err) = stdin.write_all(prompt.as_bytes()).await {
        let _ = child.kill().await;
        store.release_session_runtime_deliveries(attempt.id).await?;
        return Err(anyhow::anyhow!(
            "failed writing Session runtime prompt: {err}"
        ));
    }
    drop(stdin);

    store
        .complete_session_runtime_deliveries(attempt.id, &format!("session_runtime:{}", attempt.id))
        .await?;

    let status = child.wait().await?;
    let exit_code = status.code().map(i64::from);
    let stdout = fs::read_to_string(stdout_path).await.unwrap_or_default();
    let stderr = fs::read_to_string(stderr_path).await.unwrap_or_default();
    let provider_ref =
        known_provider_ref.or_else(|| super::launch::extract_external_session_ref(&stdout));
    let error = if status.success() {
        None
    } else {
        super::launch::extract_codex_error(&stdout)
            .or_else(|| {
                let value = stderr.trim();
                (!value.is_empty()).then(|| value.chars().take(2000).collect())
            })
            .or_else(|| Some(format!("Agent CLI exited with {:?}", exit_code)))
    };

    store
        .finish_session_runtime_attempt(attempt.id, exit_code, provider_ref, error.clone())
        .await?;
    if let Some(error) = error {
        anyhow::bail!(error);
    }
    Ok(())
}

fn build_session_runtime_prompt(
    attempt: &SessionRuntimeAttempt,
    session: &morrows_core::Session,
    summary: Option<&SessionSummaryRevision>,
    deliveries: &[morrows_core::AgentDelivery],
) -> String {
    let messages = deliveries
        .iter()
        .filter(|delivery| delivery.kind == "session_message")
        .filter_map(|delivery| delivery.payload.get("body").and_then(Value::as_str))
        .map(|body| body.chars().take(4000).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n---\n");
    let recovery_summary = summary
        .map(render_session_recovery_summary)
        .unwrap_or_else(|| {
            "(none; use session_get to reconstruct from durable history)".to_owned()
        });
    let scope = match (session.project_id, session.task_id) {
        (_, Some(task_id)) => format!(
            "This Session is scoped to Task {task_id}. Read task_get and memory_get for that Task before substantive work. This direct Session runtime is not itself a Task Run, so do not create or complete Runs unless a separate assigned Run explicitly exists."
        ),
        (Some(project_id), None) => format!(
            "This Session is scoped to Project {project_id}. Treat the Project association and Session history as durable context; do not invent a Task unless work is formally submitted."
        ),
        (None, None) => "This is a general Session with no Project or Task scope.".to_owned(),
    };
    format!(
        "You are AgentInstance {agent} started for Morrows Session {session_id}.\n{scope}\nUse the configured Morrows MCP server as the durable source of truth. The latest structured Session summary is injected below as bounded recovery context. Treat it as a recovery aid, not as a substitute for canonical records: use session_summary_get if you need the exact latest structured summary, and use session_get for paged raw history or details not covered by the summary. Process the human messages and respond with session_reply. Update session_summary_revise after materially advancing the Session.\n\nLatest structured Session recovery summary:\n{recovery_summary}\n\nPending human messages already delivered to this runtime:\n{messages}\n",
        agent = attempt.agent_instance_id,
        session_id = attempt.session_id,
        scope = scope,
        recovery_summary = recovery_summary,
        messages = if messages.is_empty() {
            "(none; inspect the Session with session_get)".to_owned()
        } else {
            messages
        },
    )
}

fn render_session_recovery_summary(summary: &SessionSummaryRevision) -> String {
    let rendered = format!(
        "Summary revision: {revision}\nCovers through message: {covers}\nGoal:\n{goal}\n\nCurrent state:\n{current_state}\n\nImportant findings (JSON):\n{important_findings}\n\nDecisions (JSON):\n{decisions}\n\nBlockers (JSON):\n{blockers}\n\nUnresolved questions (JSON):\n{unresolved_questions}\n\nNext steps (JSON):\n{next_steps}\n\nDeterministic facts (JSON):\n{deterministic_facts}",
        revision = summary.id,
        covers = summary
            .covers_until_message_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "none".to_owned()),
        goal = summary.goal,
        current_state = summary.current_state,
        important_findings = summary.important_findings,
        decisions = summary.decisions,
        blockers = summary.blockers,
        unresolved_questions = summary.unresolved_questions,
        next_steps = summary.next_steps,
        deterministic_facts = summary.deterministic_facts,
    );
    clip_session_prompt(&rendered, 12_000)
}

fn clip_session_prompt(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    let mut clipped = value.chars().take(max_chars).collect::<String>();
    clipped.push_str("\n[truncated; use session_summary_get for the complete structured summary]");
    clipped
}

async fn write_session_codebuddy_mcp(
    path: &std::path::Path,
    agent_id: Id,
    token: &str,
) -> anyhow::Result<()> {
    let morrows_url = std::env::var("MORROWS_MCP_URL")
        .or_else(|_| std::env::var("AC_MCP_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:8787/mcp".into());
    let config = json!({
        "mcpServers": {
            "morrows": {
                "type": "http",
                "url": morrows_url,
                "headers": {
                    "Authorization": format!("Bearer {token}"),
                    "X-Agent-Instance-Id": agent_id.to_string()
                },
                "description": "Morrows employee interface"
            }
        },
        "disabledMcpServers": []
    });
    fs::write(path, serde_json::to_vec_pretty(&config)?).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn session_runtime_root() -> anyhow::Result<PathBuf> {
    let configured = std::env::var("MORROWS_SESSION_RUNTIME_DIR")
        .unwrap_or_else(|_| "data/session-runtimes".into());
    let path = PathBuf::from(configured);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

async fn summary_get(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state.store.get_latest_session_summary_revision(id).await?
    )))
}

async fn summary_create(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    Json(mut input): Json<CreateSessionSummaryRevision>,
) -> Result<Json<Value>, ApiError> {
    input.session_id = id;
    Ok(Json(json!(
        state.store.create_session_summary_revision(input).await?
    )))
}

async fn summary_history(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state.store.list_session_summary_revisions(id).await?
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;

    async fn response_json(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn session_scope_can_follow_task_or_project_and_be_reassigned() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("scoped-agent", &[]).await.unwrap();
        let project = store
            .create_project(morrows_core::CreateProject {
                name: "Morrows".into(),
                description: "Agent work OS".into(),
            })
            .await
            .unwrap();
        let task = store
            .create_task(
                serde_json::from_value(json!({
                    "project_id": project.id,
                    "title": "Unify sessions",
                    "description": "Bind task execution to durable sessions"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });

        let (status, session) = response_json(
            &app,
            Request::builder()
                .method("POST")
                .uri("/sessions")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "agent_instance_id": agent.id,
                        "task_id": task.id,
                        "title": "Task chat"
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{session}");
        assert_eq!(session["task_id"], json!(task.id));
        assert_eq!(session["project_id"], json!(project.id));
        let session_id = session["id"].as_str().unwrap();

        let (status, task_sessions) = response_json(
            &app,
            Request::builder()
                .uri(format!("/sessions?task_id={}", task.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(task_sessions.as_array().unwrap().len(), 1);
        assert_eq!(task_sessions[0]["project_name"], "Morrows");
        assert_eq!(task_sessions[0]["task_title"], "Unify sessions");

        let (status, project_scoped) = response_json(
            &app,
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/scope"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"project_id": project.id, "task_id": null}).to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(project_scoped["project_id"], json!(project.id));
        assert!(project_scoped["task_id"].is_null());

        let (status, general) = response_json(
            &app,
            Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/scope"))
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"project_id": null, "task_id": null}).to_string(),
                ))
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(general["project_id"].is_null());
        assert!(general["task_id"].is_null());
    }

    #[tokio::test]
    async fn session_list_is_summary_only_and_history_is_paged() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("chat-agent", &[]).await.unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });

        let legacy_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/conversations")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy_response.status(), StatusCode::NOT_FOUND);

        let create_request = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"agent_instance_id":agent.id,"title":"Design chat"}).to_string(),
            ))
            .unwrap();
        let (status, session) = response_json(&app, create_request).await;
        assert_eq!(status, StatusCode::OK);
        let session_id = session["id"].as_str().unwrap();

        for body in ["one", "two", "three"] {
            let request = Request::builder()
                .method("POST")
                .uri(format!("/sessions/{session_id}/messages"))
                .header("content-type", "application/json")
                .body(Body::from(json!({"body":body}).to_string()))
                .unwrap();
            assert_eq!(response_json(&app, request).await.0, StatusCode::OK);
        }

        let (status, summaries) = response_json(
            &app,
            Request::builder()
                .uri("/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(summaries[0]["message_count"], 3);
        assert_eq!(summaries[0]["last_message_preview"], "three");
        assert!(summaries[0].get("messages").is_none());

        let (status, history) = response_json(
            &app,
            Request::builder()
                .uri(format!("/sessions/{session_id}/messages?limit=2"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(history["messages"].as_array().unwrap().len(), 2);
        assert_eq!(history["messages"][0]["body"], "two");
        assert_eq!(history["messages"][1]["body"], "three");
        assert_eq!(history["has_more"], true);

        let before = history["next_before"].as_str().unwrap();
        let older = response_json(
            &app,
            Request::builder()
                .uri(format!(
                    "/sessions/{session_id}/messages?limit=2&before={before}"
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .1;
        assert_eq!(older["messages"][0]["body"], "one");
        assert_eq!(older["has_more"], false);
    }

    #[tokio::test]
    async fn session_runtime_launch_delivers_message_and_persists_provider_ref() {
        use std::os::unix::fs::PermissionsExt;

        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store
            .register_agent("session-runtime-agent", &[])
            .await
            .unwrap();
        let session = store
            .create_session(CreateSession {
                agent_instance_id: agent.id,
                title: "Runtime".into(),
            })
            .await
            .unwrap();
        let message = store
            .create_human_session_message(session.id, "hello runtime")
            .await
            .unwrap();
        let summary = store
            .create_session_summary_revision(CreateSessionSummaryRevision {
                session_id: session.id,
                previous_revision_id: None,
                covers_until_message_id: Some(message.id),
                goal: "Continue the durable Session".into(),
                current_state: "Recovered state marker".into(),
                important_findings: json!(["summary finding"]),
                decisions: json!(["summary decision"]),
                blockers: json!([]),
                unresolved_questions: json!(["summary question"]),
                next_steps: json!(["summary next step"]),
                deterministic_facts: json!({"summary_fact":"present"}),
                created_by: "test".into(),
            })
            .await
            .unwrap();

        let temp = std::env::temp_dir().join(format!("morrows-session-runtime-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let prompt_path = temp.join("prompt.txt");
        let args_path = temp.join("args.txt");
        let program = temp.join("fake-codex");
        std::fs::write(
            &program,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$@" > '{}'
cat > '{}'
echo '{{"type":"thread.started","thread_id":"fake-session-thread-123"}}'
"#,
                args_path.display(),
                prompt_path.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();

        store
            .register_launch_profile(
                serde_json::from_value(json!({
                    "name": "session runtime fake codex",
                    "adapter": "codex_cli",
                    "agent_instance_id": agent.id,
                    "program": program,
                    "default_cwd": temp,
                    "enabled": true
                }))
                .unwrap(),
            )
            .await
            .unwrap();

        let attempt = store
            .create_session_runtime_attempt(
                session.id,
                StartSessionRuntime {
                    launch_profile_id: None,
                    model: Some("gpt-5.6-sol".into()),
                    reasoning_effort: Some("high".into()),
                },
            )
            .await
            .unwrap();
        execute_session_runtime(store.clone(), attempt.clone())
            .await
            .unwrap();

        let finished = store.get_session_runtime_attempt(attempt.id).await.unwrap();
        assert_eq!(finished.status, "completed");
        assert_eq!(
            finished.provider_session_ref.as_deref(),
            Some("fake-session-thread-123")
        );

        let history = store
            .session_history(session.id, None, None, 20)
            .await
            .unwrap();
        let delivered = history
            .messages
            .iter()
            .find(|item| item.id == message.id)
            .unwrap();
        assert_eq!(delivered.delivery_status.as_deref(), Some("delivered"));

        let prompt = std::fs::read_to_string(&prompt_path).unwrap();
        assert!(prompt.contains(&session.id.to_string()));
        assert!(prompt.contains("hello runtime"));
        assert!(prompt.contains(&summary.id.to_string()));
        assert!(prompt.contains("Continue the durable Session"));
        assert!(prompt.contains("Recovered state marker"));
        assert!(prompt.contains("summary_fact"));
        assert!(prompt.contains("session_summary_get"));
        assert!(prompt.contains("session_get"));
        assert_eq!(finished.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(finished.reasoning_effort.as_deref(), Some("high"));
        let args = std::fs::read_to_string(&args_path).unwrap();
        assert!(args.contains("gpt-5.6-sol"));
        assert!(args.contains("model_reasoning_effort=\"high\""));

        let credentials = store.list_agent_credentials(agent.id).await.unwrap();
        let runtime_credential = credentials
            .iter()
            .find(|credential| credential.kind == "session_runtime")
            .unwrap();
        assert_eq!(runtime_credential.session_id, Some(session.id));
        assert!(runtime_credential.revoked_at.is_some());

        let _ = std::fs::remove_dir_all(temp);
        let _ = std::fs::remove_dir_all(
            std::path::PathBuf::from("data/session-runtimes").join(attempt.id.to_string()),
        );
    }

    #[tokio::test]
    async fn session_summary_rest_routes_support_lifecycle_and_history() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("summary-agent", &[]).await.unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });

        let create_request = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"agent_instance_id": agent.id, "title": "Summary Test"}).to_string(),
            ))
            .unwrap();
        let (status, session) = response_json(&app, create_request).await;
        assert_eq!(status, StatusCode::OK);
        let session_id = session["id"].as_str().unwrap();

        // 1. Initial GET summary returns null
        let (status, initial_summary) = response_json(
            &app,
            Request::builder()
                .uri(format!("/sessions/{session_id}/summary"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(initial_summary.is_null());

        // 2. POST create first summary revision
        let post_summary1 = Request::builder()
            .method("POST")
            .uri(format!("/sessions/{session_id}/summary"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "goal": "Initial assessment",
                    "current_state": "Analyzing logs",
                    "deterministic_facts": {"files_inspected": 2},
                    "created_by": "system"
                })
                .to_string(),
            ))
            .unwrap();
        let (status, summary1) = response_json(&app, post_summary1).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(summary1["goal"], "Initial assessment");
        assert_eq!(summary1["current_state"], "Analyzing logs");
        let rev1_id = summary1["id"].as_str().unwrap();

        // 3. GET summary returns the latest (revision 1)
        let (status, latest1) = response_json(
            &app,
            Request::builder()
                .uri(format!("/sessions/{session_id}/summary"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(latest1["id"], rev1_id);

        // 4. POST create second summary revision referencing previous
        let post_summary2 = Request::builder()
            .method("POST")
            .uri(format!("/sessions/{session_id}/summary"))
            .header("content-type", "application/json")
            .body(Body::from(
                json!({
                    "previous_revision_id": rev1_id,
                    "goal": "Fix crash",
                    "current_state": "Patch applied",
                    "created_by": "agent"
                })
                .to_string(),
            ))
            .unwrap();
        let (status, summary2) = response_json(&app, post_summary2).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(summary2["goal"], "Fix crash");
        assert_eq!(summary2["previous_revision_id"], rev1_id);

        // 5. GET summary/history returns both in reverse chronological order
        let (status, history) = response_json(
            &app,
            Request::builder()
                .uri(format!("/sessions/{session_id}/summary/history"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let list = history.as_array().unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["id"], summary2["id"]);
        assert_eq!(list[1]["id"], rev1_id);
    }
}
