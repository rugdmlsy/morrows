use super::*;
use crate::lsm::{AgentBinding, LsmControl};
use axum::extract::Query;
use morrows_core::*;
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
        .route("/launch-attempts/{id}/stop", post(launch_stop))
        .route("/runs/{id}/restart", post(run_restart))
        .route("/runs/{id}/cancel", post(run_cancel))
        .route("/runs/{id}/execution", get(run_execution))
        .route(
            "/runs/{id}/evidence",
            get(run_evidence).post(attach_run_evidence),
        )
        .route("/runs/{id}/jobs/{job_id}/output", get(run_job_output))
        .route(
            "/launch-attempts/{id}/instructions",
            get(instruction_list).post(instruction_send),
        )
        .route("/launch-attempts/external/accept", post(external_accept))
        .route(
            "/agent-instances/{id}/external-launches",
            get(external_launches),
        )
        .route("/tasks/{id}/launch-attempts", get(task_launches))
        .route("/tasks/{id}/launch-instructions", get(task_instructions))
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

async fn launch_stop(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    if let Some(run_id) = s.store.get_launch_attempt(id).await?.run_id
        && s.store.has_lsm_runtime(run_id).await?
    {
        return Err(ApiError(DomainError::Conflict(
            "LSM-bound Runs must be cancelled through /runs/{id}/cancel".into(),
        )));
    }
    Ok(Json(json!(s.store.stop_launch_attempt(id).await?)))
}

async fn run_restart(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    let run = s.store.get_run(id).await?;
    if run.status != "interrupted" {
        return Err(ApiError(DomainError::Conflict(format!(
            "Run is {}",
            run.status
        ))));
    }
    if s.store.run_lsm_binding(id).await?.is_none() && s.store.has_lsm_runtime(id).await? {
        let control = LsmControl::from_env()
            .map_err(|err| ApiError(DomainError::Storage(err.to_string())))?
            .ok_or_else(|| {
                ApiError(DomainError::Conflict(
                    "LSM control is not configured".into(),
                ))
            })?;
        let task = s.store.get_task(run.task_id).await?;
        control
            .provision_run(&s.store, id, &task.title)
            .await
            .map_err(|err| ApiError(DomainError::Storage(err.to_string())))?;
    }
    Ok(Json(json!(s.store.enqueue_run_restart(id).await?)))
}

async fn run_cancel(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    // Persist cancellation first. Revocation and cleanup are retryable side effects
    // of that durable intent, including when LSM is temporarily unavailable.
    let run = s.store.request_run_cancel(id).await?;
    match LsmControl::from_env() {
        Ok(Some(control)) => {
            if let Err(err) = control.revoke_for_run(&s.store, id).await {
                tracing::warn!(%id, %err, "capability revocation pending after Run cancel");
            }
        }
        Ok(None) => tracing::warn!(%id, "LSM control is not configured; Run cleanup pending"),
        Err(err) => tracing::warn!(%id, %err, "LSM control is invalid; Run cleanup pending"),
    }
    Ok(Json(json!(run)))
}

async fn run_execution(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    let binding = s.store.run_lsm_binding(id).await?;
    let Some(binding) = binding else {
        return Ok(Json(json!({"binding":null,"observation":null})));
    };
    let observation = match LsmControl::from_env() {
        Ok(Some(control)) => match control.observe(&binding.logical_session_id).await {
            Ok(data) => data,
            Err(err) => json!({"error":err.to_string()}),
        },
        Ok(None) => json!({"error":"LSM control is not configured"}),
        Err(err) => json!({"error":err.to_string()}),
    };
    let evidence = s.store.run_execution_evidence(id).await?;
    Ok(Json(
        json!({"binding":binding,"observation":observation,"evidence":evidence}),
    ))
}

async fn run_evidence(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.run_execution_evidence(id).await?)))
}

async fn attach_run_evidence(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<AttachExecutionEvidence>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.attach_run_execution_evidence(id, input).await?
    )))
}

#[derive(Deserialize)]
struct JobOutputQuery {
    machine: Option<String>,
}

async fn run_job_output(
    State(s): State<AppState>,
    Path((id, job_id)): Path<(Id, String)>,
    Query(query): Query<JobOutputQuery>,
) -> Result<Json<Value>, ApiError> {
    let binding = s
        .store
        .run_lsm_binding(id)
        .await?
        .ok_or_else(|| ApiError(DomainError::NotFound("Run LSM binding".into())))?;
    let control = LsmControl::from_env()
        .map_err(|err| ApiError(DomainError::Storage(err.to_string())))?
        .ok_or_else(|| {
            ApiError(DomainError::Conflict(
                "LSM control is not configured".into(),
            ))
        })?;
    let output = control
        .job_tail(
            &binding.logical_session_id,
            &job_id,
            query.machine.as_deref().unwrap_or("local"),
        )
        .await
        .map_err(|err| ApiError(DomainError::Storage(err.to_string())))?;
    Ok(Json(output))
}

pub async fn runtime_sweep(store: Store) {
    let Some(control) = (match LsmControl::from_env() {
        Ok(control) => control,
        Err(err) => {
            tracing::error!(%err, "LSM control configuration is invalid");
            return;
        }
    }) else {
        return;
    };
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    loop {
        interval.tick().await;
        if let Ok(runs) = store.unbound_lsm_runs().await {
            for run_id in runs {
                let run = match store.get_run(run_id).await {
                    Ok(run) => run,
                    Err(err) => {
                        tracing::warn!(%run_id, %err, "cannot read unbound LSM Run");
                        continue;
                    }
                };
                let task = match store.get_task(run.task_id).await {
                    Ok(task) => task,
                    Err(err) => {
                        tracing::warn!(%run_id, %err, "cannot read LSM Run Task");
                        continue;
                    }
                };
                if let Err(err) = control.provision_run(&store, run_id, &task.title).await {
                    tracing::warn!(%run_id, %err, "LSM provisioning replay remains pending");
                }
            }
        }
        if let Err(err) = store.renew_interrupted_assignments().await {
            tracing::error!(%err, "failed renewing interrupted Assignments");
        }
        if let Ok(runs) = store.interrupted_lsm_runs().await {
            for run_id in runs {
                if let Err(err) = control.revoke_for_run(&store, run_id).await {
                    tracing::warn!(%run_id, %err, "failed revoking interrupted Agent capability");
                }
            }
        }
        if let Ok(runs) = store.due_interrupted_runs().await {
            for run_id in runs {
                if let Err(err) = store.begin_expired_cleanup(run_id).await {
                    tracing::error!(%run_id, %err, "failed entering runtime cleanup");
                }
            }
        }
        if let Ok(runs) = store.pending_lsm_cleanup_runs().await {
            for run_id in runs {
                if store
                    .has_active_launch_attempt(run_id)
                    .await
                    .unwrap_or(true)
                {
                    continue;
                }
                let Some(binding) = store.run_lsm_binding(run_id).await.ok().flatten() else {
                    continue;
                };
                if let Err(err) = control.revoke_for_run(&store, run_id).await {
                    tracing::warn!(%run_id, %err, "failed revoking Agent capability before cleanup");
                    continue;
                }
                match control
                    .cleanup_run(&store, run_id, &binding.logical_session_id, false)
                    .await
                {
                    Ok(true) => {
                        if let Err(err) = store.complete_lsm_cleanup(run_id).await {
                            tracing::error!(%run_id, %err, "failed recording completed cleanup");
                        }
                    }
                    Ok(false) => tracing::warn!(%run_id, "LSM cleanup remains pending"),
                    Err(err) => tracing::warn!(%run_id, %err, "LSM cleanup failed; will retry"),
                }
            }
        }
        if let Ok(runs) = store.completed_lsm_runs().await {
            for run_id in runs {
                if store
                    .has_active_launch_attempt(run_id)
                    .await
                    .unwrap_or(true)
                {
                    continue;
                }
                let Some(binding) = store.run_lsm_binding(run_id).await.ok().flatten() else {
                    continue;
                };
                if let Err(err) = control.revoke_for_run(&store, run_id).await {
                    tracing::warn!(%run_id, %err, "failed revoking completed Agent capability");
                    continue;
                }
                match control
                    .cleanup_run(&store, run_id, &binding.logical_session_id, true)
                    .await
                {
                    Ok(true) => {
                        if let Err(err) = store.mark_lsm_terminalized(run_id).await {
                            tracing::error!(%run_id, %err, "failed recording Session finish");
                        }
                    }
                    Ok(false) => tracing::warn!(%run_id, "LSM Session finish remains pending"),
                    Err(err) => {
                        tracing::warn!(%run_id, %err, "LSM Session finish failed; will retry")
                    }
                }
            }
        }
    }
}

async fn instruction_list(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.launch_instructions(id).await?)))
}

async fn instruction_send(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(body): Json<SendLaunchInstruction>,
) -> Result<Json<Value>, ApiError> {
    if body.launch_attempt_id != id {
        return Err(ApiError(DomainError::InvalidInput(
            "attempt id mismatch".into(),
        )));
    }
    Ok(Json(json!(
        s.store.send_launch_instruction(body, None).await?
    )))
}

async fn external_accept(
    State(s): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(input): Json<AcceptExternalLaunch>,
) -> Result<Json<Value>, ApiError> {
    let agent = headers
        .get("X-Agent-Instance-Id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Id::parse_str(value).ok())
        .ok_or_else(|| {
            ApiError(DomainError::InvalidInput(
                "X-Agent-Instance-Id required".into(),
            ))
        })?;
    Ok(Json(json!(
        s.store.accept_external_launch(input, agent).await?
    )))
}

async fn external_launches(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.pending_external_launches(id).await?)))
}

async fn task_launches(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.task_launch_attempts(id).await?)))
}

async fn task_instructions(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.task_launch_instructions(id).await?)))
}

pub async fn delivery_worker_loop(store: Store) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        interval.tick().await;
        let candidates = match store.delivery_resume_candidates().await {
            Ok(value) => value,
            Err(err) => {
                tracing::error!(%err, "Agent delivery resume scan failed");
                continue;
            }
        };
        for run_id in candidates {
            match store.enqueue_run_delivery_resume(run_id).await {
                Ok(attempt) => tracing::info!(
                    %run_id,
                    launch_attempt_id = %attempt.id,
                    "queued Codex continuation for pending Agent delivery"
                ),
                Err(DomainError::Conflict(_)) => {}
                Err(err) => tracing::error!(%run_id, %err, "failed to queue delivery continuation"),
            }
        }
    }
}

pub async fn worker_loop(store: Store) {
    let mut interval = tokio::time::interval(std::time::Duration::from_millis(500));
    loop {
        interval.tick().await;
        if let Err(err) = store.reconcile_external_launches().await {
            tracing::error!(%err, "external launch reconciliation failed");
        }
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
    let control = LsmControl::from_env()?;
    let execution = match store
        .begin_launch_attempt_with_lsm(job.attempt.id, control.as_ref().map(LsmControl::subject))
        .await
    {
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
        "codebuddy_cli" => execute_codebuddy(store, execution).await,
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

async fn execute_codebuddy(store: Store, execution: LaunchExecution) -> anyhow::Result<()> {
    let root = launch_root()?;
    execute_codebuddy_with_root(store, execution, root).await
}

struct SecretFileGuard(PathBuf);

impl Drop for SecretFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

async fn write_codebuddy_mcp_config(
    path: &FsPath,
    execution: &LaunchExecution,
    agent_binding: Option<&AgentBinding>,
    morrows_token: &str,
) -> anyhow::Result<SecretFileGuard> {
    let morrows_url = std::env::var("MORROWS_MCP_URL")
        .or_else(|_| std::env::var("AC_MCP_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:8787/mcp".into());
    let mut servers = serde_json::Map::new();
    servers.insert(
        "morrows".into(),
        json!({
            "type": "http",
            "url": morrows_url,
            "headers": {
                "Authorization": format!("Bearer {morrows_token}"),
                "X-Agent-Instance-Id": execution.attempt.agent_instance_id.to_string(),
            },
            "description": "Morrows employee interface",
        }),
    );
    if let Some(binding) = agent_binding {
        servers.insert(
            "lsm".into(),
            json!({
                "type": "http",
                "url": binding.mcp_url,
                "headers": {
                    "X-LSM-Session-Capability": binding.capability,
                },
                "description": "LSM execution interface",
            }),
        );
    }
    let config = json!({
        "mcpServers": servers,
        "disabledMcpServers": [],
    });
    fs::write(path, serde_json::to_vec_pretty(&config)?).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(SecretFileGuard(path.to_path_buf()))
}

pub(crate) fn codebuddy_args(
    profile: &LaunchProfile,
    mcp_config_path: &str,
    session_ref: &str,
    resume: bool,
) -> Vec<String> {
    let mut args = vec![
        "--print".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--input-format".into(),
        "text".into(),
        "--dangerously-skip-permissions".into(),
        "--tools".into(),
        "ToolSearch,DeferExecuteTool".into(),
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        mcp_config_path.into(),
    ];
    if let Some(model) = profile.model.as_deref() {
        args.extend(["--model".into(), model.into()]);
    }
    if resume {
        args.extend(["--resume".into(), session_ref.into()]);
    } else {
        args.extend(["--session-id".into(), session_ref.into()]);
    }
    args
}

async fn execute_codebuddy_with_root(
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

    let control = LsmControl::from_env()?;
    let agent_binding = if let Some(control) = &control {
        Some(
            control
                .provision_agent(&store, execution.run.id, &execution.task.title)
                .await?,
        )
    } else {
        None
    };

    let morrows_credential = store
        .issue_runtime_credential(
            execution.attempt.agent_instance_id,
            execution.run.id,
            &format!("codebuddy launch {}", execution.attempt.id),
            3600,
        )
        .await?;

    let dir = root.join(execution.attempt.id.to_string());
    fs::create_dir_all(&dir).await?;
    let stdout_path = dir.join("stdout.jsonl");
    let stderr_path = dir.join("stderr.log");
    let mcp_config_path = dir.join("codebuddy-mcp.json");
    let _mcp_config_guard = write_codebuddy_mcp_config(
        &mcp_config_path,
        &execution,
        agent_binding.as_ref(),
        &morrows_credential.token,
    )
    .await?;
    let stdout_file = std::fs::File::create(&stdout_path)?;
    let stderr_file = std::fs::File::create(&stderr_path)?;

    let resume_session = resolve_launch_provider_ref(&store, &execution).await?;
    let session_ref = resume_session
        .clone()
        .unwrap_or_else(|| format!("morrows-{}", execution.attempt.id));
    let args = codebuddy_args(
        &execution.profile,
        mcp_config_path.to_string_lossy().as_ref(),
        &session_ref,
        resume_session.is_some(),
    );

    let mut command = Command::new(&execution.profile.program);
    command
        .args(&args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .kill_on_drop(true);
    command.env_remove("MORROWS_LSM_CONTROL_KEY");
    command.env_remove("LOCAL_SHELL_MCP_CONTROL_API_KEY");

    crate::configure_memory_cli(&mut command);
    command.env(
        "MORROWS_AGENT_AUTHORIZATION",
        format!("Bearer {}", morrows_credential.token),
    );
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            if let Some(control) = &control {
                let _ = control.revoke_for_run(&store, execution.run.id).await;
            }
            return Err(err.into());
        }
    };
    let pid = child.id().map(i64::from);
    if let Err(err) = store
        .mark_launch_running(
            execution.attempt.id,
            pid,
            stdout_path.to_string_lossy().into_owned(),
            stderr_path.to_string_lossy().into_owned(),
        )
        .await
    {
        let _ = child.kill().await;
        if let Some(control) = &control {
            let _ = control.revoke_for_run(&store, execution.run.id).await;
        }
        return Err(err.into());
    }

    let claimed_deliveries = store
        .claim_agent_deliveries_for_launch(execution.attempt.id, 50)
        .await?;
    let mut prompt = build_prompt(&execution);
    let delivery_prompt = build_delivery_prompt(&execution, &claimed_deliveries);
    if !delivery_prompt.is_empty() {
        prompt.push_str(&delivery_prompt);
    }
    if let Some(binding) = &agent_binding {
        prompt.push_str(&format!(
            "\nLSM execution context: {}. Use only this Logical Session for LSM calls. Morrows owns Session lifecycle; do not start, finish, cancel, or delete it.\n",
            binding.logical_session_id,
        ));
    }

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(err) = stdin.write_all(prompt.as_bytes()).await {
            let _ = store
                .release_claimed_agent_deliveries(execution.attempt.id)
                .await;
            let _ = child.kill().await;
            if let Some(control) = &control {
                let _ = control.revoke_for_run(&store, execution.run.id).await;
            }
            let message = format!("failed writing CodeBuddy launcher prompt to stdin: {err}");
            store
                .finish_launch_attempt(
                    execution.attempt.id,
                    None,
                    Some(session_ref.clone()),
                    Some(message.clone()),
                )
                .await?;
            anyhow::bail!(message);
        }
        drop(stdin);
        if !claimed_deliveries.is_empty()
            && let Err(err) = store
                .complete_claimed_agent_deliveries(
                    execution.attempt.id,
                    &format!("codebuddy_cli:{}", execution.attempt.id),
                )
                .await
        {
            tracing::warn!(
                launch_attempt_id = %execution.attempt.id,
                %err,
                "CodeBuddy received delivery prompt but delivery acknowledgement could not be persisted"
            );
        }
    } else {
        let _ = store
            .release_claimed_agent_deliveries(execution.attempt.id)
            .await;
        let _ = child.kill().await;
        if let Some(control) = &control {
            let _ = control.revoke_for_run(&store, execution.run.id).await;
        }
        let message = "CodeBuddy launcher stdin is unavailable".to_owned();
        store
            .finish_launch_attempt(
                execution.attempt.id,
                None,
                Some(session_ref.clone()),
                Some(message.clone()),
            )
            .await?;
        anyhow::bail!(message);
    }

    let mut poll = tokio::time::interval(std::time::Duration::from_millis(250));
    let status = loop {
        tokio::select! {
            result = child.wait() => break result,
            _ = poll.tick() => {
                if store.launch_stop_requested(execution.attempt.id).await? {
                    let _ = child.kill().await;
                    if let Some(control) = &control {
                        control.revoke_for_run(&store, execution.run.id).await?;
                    }
                    store.finish_launch_attempt(
                        execution.attempt.id,
                        None,
                        Some(session_ref.clone()),
                        Some("run_cancel_requested".into()),
                    ).await?;
                    return Ok(());
                }
            }
        }
    };
    let status = match status {
        Ok(value) => value,
        Err(err) => {
            if let Some(control) = &control {
                let _ = control.revoke_for_run(&store, execution.run.id).await;
            }
            let message = format!("failed waiting for CodeBuddy process: {err}");
            store
                .finish_launch_attempt(
                    execution.attempt.id,
                    None,
                    Some(session_ref.clone()),
                    Some(message.clone()),
                )
                .await?;
            anyhow::bail!(message);
        }
    };
    let exit_code = status.code().map(i64::from);
    let stderr = fs::read_to_string(&stderr_path).await.unwrap_or_default();
    let error = if status.success() {
        None
    } else {
        let stdout = fs::read_to_string(&stdout_path).await.unwrap_or_default();
        extract_codex_error(&stdout)
            .or_else(|| {
                let message = stderr.trim();
                (!message.is_empty()).then(|| clip(message, 2000))
            })
            .map(|message| format!("codebuddy: {message}"))
            .or_else(|| {
                Some(format!(
                    "CodeBuddy process exited with {}",
                    exit_code
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "signal".into())
                ))
            })
    };
    if let Some(control) = &control {
        control.revoke_for_run(&store, execution.run.id).await?;
    }
    store
        .finish_launch_attempt(execution.attempt.id, exit_code, Some(session_ref), error)
        .await?;
    Ok(())
}

pub(crate) fn resolve_codex_home(account: Option<&Account>) -> anyhow::Result<Option<PathBuf>> {
    let Some(account) = account else {
        return Ok(None);
    };
    match (
        account.credential_kind.as_deref(),
        account.credential_ref.as_deref(),
    ) {
        (None, None) => Ok(None),
        (Some("codex_home"), Some(reference)) => {
            let path = if reference == "~" {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .ok_or_else(|| anyhow::anyhow!("HOME is unavailable for credential_ref ~"))?
            } else if let Some(rest) = reference.strip_prefix("~/") {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .ok_or_else(|| {
                        anyhow::anyhow!("HOME is unavailable for credential_ref {reference}")
                    })?
                    .join(rest)
            } else {
                PathBuf::from(reference)
            };
            if !path.is_absolute() {
                anyhow::bail!(
                    "codex_home credential_ref must be absolute or start with ~/: {reference}"
                );
            }
            Ok(Some(path))
        }
        (Some(kind), Some(_)) => anyhow::bail!("unsupported credential kind for Codex: {kind}"),
        _ => anyhow::bail!("Account credential_kind and credential_ref must be set together"),
    }
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
    let codex_home = resolve_codex_home(execution.account.as_ref())?;
    if let Some(home) = codex_home.as_deref()
        && !home.is_dir()
    {
        anyhow::bail!(
            "credential_ref CODEX_HOME does not exist on this runtime machine: {}",
            home.display()
        );
    }

    let control = LsmControl::from_env()?;
    let agent_binding = if let Some(control) = &control {
        Some(
            control
                .provision_agent(&store, execution.run.id, &execution.task.title)
                .await?,
        )
    } else {
        None
    };
    let morrows_credential = store
        .issue_runtime_credential(
            execution.attempt.agent_instance_id,
            execution.run.id,
            &format!("codex launch {}", execution.attempt.id),
            3600,
        )
        .await?;

    let dir = root.join(execution.attempt.id.to_string());
    fs::create_dir_all(&dir).await?;
    let stdout_path = dir.join("stdout.jsonl");
    let stderr_path = dir.join("stderr.log");
    let last_message_path = dir.join("last-message.txt");
    let stdout_file = std::fs::File::create(&stdout_path)?;
    let stderr_file = std::fs::File::create(&stderr_path)?;

    let resume_session = resolve_launch_provider_ref(&store, &execution).await?;

    let mut args = codex_args(
        &execution.profile,
        &cwd,
        last_message_path.to_string_lossy().as_ref(),
        resume_session.as_deref(),
    );
    inject_morrows_config(&mut args);
    if let Some(binding) = &agent_binding {
        inject_lsm_config(&mut args, binding);
    }
    let mut command = Command::new(&execution.profile.program);
    command
        .args(&args)
        .current_dir(&cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout_file))
        .stderr(Stdio::from(stderr_file))
        .kill_on_drop(true);
    command.env_remove("MORROWS_LSM_CONTROL_KEY");
    command.env_remove("LOCAL_SHELL_MCP_CONTROL_API_KEY");
    command.env_remove("MORROWS_AGENT_AUTHORIZATION");
    command.env_remove("MORROWS_AGENT_INSTANCE_ID");
    crate::configure_memory_cli(&mut command);
    command.env(
        "MORROWS_AGENT_AUTHORIZATION",
        format!("Bearer {}", morrows_credential.token),
    );
    command.env(
        "MORROWS_AGENT_INSTANCE_ID",
        execution.attempt.agent_instance_id.to_string(),
    );
    if let Some(session_id) = execution.attempt.session_id {
        command.env("MORROWS_SESSION_ID", session_id.to_string());
    }
    if let Some(binding) = &agent_binding {
        command.env("MORROWS_LSM_CAPABILITY", &binding.capability);
    }
    if let Some(home) = codex_home.as_deref() {
        command.env("CODEX_HOME", home);
    }
    let mut child = command.spawn()?;
    let pid = child.id().map(i64::from);
    if let Err(err) = store
        .mark_launch_running(
            execution.attempt.id,
            pid,
            stdout_path.to_string_lossy().into_owned(),
            stderr_path.to_string_lossy().into_owned(),
        )
        .await
    {
        let _ = child.kill().await;
        if let Some(control) = &control
            && let Err(revoke_err) = control.revoke_for_run(&store, execution.run.id).await
        {
            tracing::warn!(run_id=%execution.run.id, %revoke_err,
                "failed revoking capability after launch was fenced");
        }
        return Err(err.into());
    }

    let claimed_deliveries = store
        .claim_agent_deliveries_for_launch(execution.attempt.id, 50)
        .await?;
    let mut prompt = build_prompt(&execution);
    let delivery_prompt = build_delivery_prompt(&execution, &claimed_deliveries);
    if !delivery_prompt.is_empty() {
        prompt.push_str(&delivery_prompt);
    }
    if let Some(binding) = &agent_binding {
        prompt.push_str(&format!(
            "\nLSM execution context: {}. Use only this Logical Session for LSM calls. Morrows owns Session lifecycle; do not start, finish, cancel, or delete it. LSM will reject old Session IDs from resumed Session history.\n",
            binding.logical_session_id,
        ));
    }
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(err) = stdin.write_all(prompt.as_bytes()).await {
            let _ = store
                .release_claimed_agent_deliveries(execution.attempt.id)
                .await;
            let _ = child.kill().await;
            let message = format!("failed writing launcher prompt to stdin: {err}");
            store
                .finish_launch_attempt(execution.attempt.id, None, None, Some(message.clone()))
                .await?;
            anyhow::bail!(message);
        }
        drop(stdin);
        if !claimed_deliveries.is_empty()
            && let Err(err) = store
                .complete_claimed_agent_deliveries(
                    execution.attempt.id,
                    &format!("codex_cli:{}", execution.attempt.id),
                )
                .await
        {
            tracing::warn!(
                launch_attempt_id = %execution.attempt.id,
                %err,
                "Codex received delivery prompt but delivery acknowledgement could not be persisted"
            );
        }
    } else {
        let _ = store
            .release_claimed_agent_deliveries(execution.attempt.id)
            .await;
        let _ = child.kill().await;
        let message = "launcher stdin is unavailable".to_owned();
        store
            .finish_launch_attempt(execution.attempt.id, None, None, Some(message.clone()))
            .await?;
        anyhow::bail!(message);
    }

    // Poll the durable cancellation state while this worker owns the child. This also
    // handles stop requests from another API connection without sharing process handles.
    let mut poll = tokio::time::interval(std::time::Duration::from_millis(250));
    let status = loop {
        tokio::select! {
            result = child.wait() => break result,
            _ = poll.tick() => {
                if store.launch_stop_requested(execution.attempt.id).await? {
                    let _ = child.kill().await;
                    if let Some(control) = &control {
                        control.revoke_for_run(&store, execution.run.id).await?;
                    }
                    store.finish_launch_attempt(execution.attempt.id, None, None,
                        Some("run_cancel_requested".into())).await?;
                    return Ok(());
                }
            }
        }
    };
    let status = match status {
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
    if let Some(control) = &control {
        control.revoke_for_run(&store, execution.run.id).await?;
    }
    store
        .finish_launch_attempt(execution.attempt.id, exit_code, session_ref, error)
        .await?;
    Ok(())
}

pub(crate) fn inject_morrows_config(args: &mut Vec<String>) {
    let morrows_url = std::env::var("MORROWS_MCP_URL")
        .or_else(|_| std::env::var("AC_MCP_URL"))
        .unwrap_or_else(|_| "http://127.0.0.1:8787/mcp".into());
    let overrides = [
        format!("mcp_servers.morrows.url=\"{morrows_url}\""),
        "mcp_servers.morrows.env_http_headers.Authorization=\"MORROWS_AGENT_AUTHORIZATION\""
            .to_owned(),
        "mcp_servers.morrows.env_http_headers.X-Agent-Instance-Id=\"MORROWS_AGENT_INSTANCE_ID\""
            .to_owned(),
    ];
    for value in overrides.into_iter().rev() {
        args.insert(1, value);
        args.insert(1, "-c".into());
    }
}

fn inject_lsm_config(args: &mut Vec<String>, binding: &AgentBinding) {
    let overrides = [
        format!("mcp_servers.lsm.url=\"{}\"", binding.mcp_url),
        "mcp_servers.lsm.env_http_headers.X-LSM-Session-Capability=\"MORROWS_LSM_CAPABILITY\""
            .to_owned(),
    ];
    for value in overrides.into_iter().rev() {
        args.insert(1, value);
        args.insert(1, "-c".into());
    }
}

fn launch_root() -> anyhow::Result<PathBuf> {
    let configured = std::env::var("MORROWS_LAUNCH_DIR")
        .or_else(|_| std::env::var("AC_LAUNCH_DIR"))
        .unwrap_or_else(|_| "data/launches".into());
    let path = PathBuf::from(configured);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

pub(crate) fn codex_args(
    profile: &LaunchProfile,
    cwd: &str,
    last_message_path: &str,
    resume_session: Option<&str>,
) -> Vec<String> {
    if let Some(session) = resume_session {
        let mut args = vec![
            "exec".into(),
            "resume".into(),
            "--json".into(),
            "-o".into(),
            last_message_path.into(),
        ];
        if let Some(model) = profile.model.as_deref() {
            args.extend(["-m".into(), model.into()]);
        }
        args.extend([session.into(), "-".into()]);
        return args;
    }
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

async fn resolve_launch_provider_ref(
    store: &Store,
    execution: &LaunchExecution,
) -> anyhow::Result<Option<String>> {
    if let Some(previous_id) = execution.attempt.resume_from_attempt_id {
        if let Some(provider_ref) = store
            .get_launch_attempt(previous_id)
            .await?
            .external_session_ref
        {
            return Ok(Some(provider_ref));
        }
    }
    let Some(session_id) = execution.attempt.session_id else {
        return Ok(None);
    };
    Ok(store
        .latest_session_provider_ref(session_id, execution.profile.id)
        .await?)
}

fn build_prompt(execution: &LaunchExecution) -> String {
    let context = execution.context.as_ref();
    let constraints = context
        .map(|value| clip(&value.constraints.to_string(), 6000))
        .unwrap_or_default();
    let instructions = execution
        .instructions
        .iter()
        .map(|item| clip(&item.body, 3000))
        .collect::<Vec<_>>()
        .join("\n---\n");
    format!(
        "You are an executor launched by Morrows.\n\
AgentInstance: {agent}\n\
Task ID: {task_id}\n\
Assignment ID: {assignment_id}\n\
Run ID: {run_id}\n\
Morrows Session ID: {session_id}\n\
\nUse the configured Morrows MCP server as the durable source of truth. The Assignment and Run already exist; do not claim the task or start another Run. This execution is attached to the durable Morrows Session shown above; use session_get for its conversation history, session_reply for human-facing replies, and session_summary_revise after materially advancing it. Before substantial work, read task_context for Task ID {task_id}. Read instructions_get for management updates. Use run_checkpoint for lightweight latest-state updates and run_milestone for substantive phase boundaries, before long/risky operations, under token/provider budget pressure, and before handoff. A handoff must cite the latest milestone and its captured evidence. If the task is fully complete, use run_completion_check and call run_complete for Run ID {run_id}. If blocked or incomplete, persist a milestone/checkpoint with the blocker, exact next step, ordered next plan and execution locations before exiting.\n\
\nTask title:\n{title}\n\
\nContext preparation and source handling:\n{context_capture}\n{execution_workflow}\n\
\nTask description:\n{description}\n\
\nContext goal:\n{goal}\n\
\nContext background (bounded; read task_context for the full original):\n{background}\n\
\nContext summary:\n{summary}\n\
\nContext constraints (JSON, bounded):\n{constraints}\n\
\nInstructions received before launch:\n{instructions}\n",
        agent = execution.attempt.agent_instance_id,
        task_id = execution.task.id,
        assignment_id = execution.attempt.assignment_id,
        run_id = execution.run.id,
        session_id = execution
            .attempt
            .session_id
            .map(|value| value.to_string())
            .unwrap_or_else(|| "(legacy-unbound)".into()),
        title = clip(&execution.task.title, 2000),
        context_capture = include_str!("context_capture_instructions.md"),
        execution_workflow = include_str!("execution_workflow_instructions.md"),
        description = clip(&execution.task.description, 6000),
        goal = context.map(|v| clip(&v.goal, 4000)).unwrap_or_default(),
        background = context
            .map(|v| clip(&v.background, 6000))
            .unwrap_or_default(),
        summary = context
            .map(|v| clip(&v.current_summary, 6000))
            .unwrap_or_default(),
        instructions = clip(&instructions, 6000),
    )
}

fn build_delivery_prompt(execution: &LaunchExecution, deliveries: &[AgentDelivery]) -> String {
    if deliveries.is_empty() {
        return String::new();
    }
    let instruction_ids = execution
        .instructions
        .iter()
        .map(|instruction| instruction.id)
        .collect::<std::collections::HashSet<_>>();
    let mut items = Vec::new();
    for delivery in deliveries {
        if delivery.kind == "launch_instruction" && instruction_ids.contains(&delivery.source_id) {
            continue;
        }
        match delivery.kind.as_str() {
            "session_message" => {
                let session_id = delivery
                    .payload
                    .get("session_id")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                let body = delivery
                    .payload
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                items.push(format!(
                    "Direct company message in Session {session_id}:\n{}\nUse session_get for context and session_reply when appropriate.",
                    clip(body, 3000)
                ));
            }
            "launch_instruction" => {
                let body = delivery
                    .payload
                    .get("body")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                items.push(format!("Management instruction:\n{}", clip(body, 3000)));
            }
            other => items.push(format!(
                "Morrows delivery {other}: {}",
                clip(&delivery.payload.to_string(), 3000)
            )),
        }
    }
    if items.is_empty() {
        return String::new();
    }
    format!(
        "\n\nPending Morrows deliveries received for this turn. Process these before continuing stale work:\n{}\n",
        clip(&items.join("\n---\n"), 12000)
    )
}

fn clip(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn extract_external_session_ref(stdout: &str) -> Option<String> {
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

pub(crate) fn extract_codex_error(stdout: &str) -> Option<String> {
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
    async fn cancel_rest_persists_intent_before_lsm_revoke() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = make_agent(&store).await;
        let task = store
            .create_task(input(json!({"title":"cancel before LSM binding"})))
            .await
            .unwrap();
        let assignment = store
            .claim_task(task.id, agent.id, "executor", 600)
            .await
            .unwrap();
        let profile = store
            .register_launch_profile(input(json!({
                "name":"cancel test codex", "adapter":"codex_cli",
                "agent_instance_id":agent.id, "program":"/bin/echo",
                "default_cwd":std::env::temp_dir()
            })))
            .await
            .unwrap();
        let attempt = store
            .enqueue_launch(input(json!({
                "assignment_id":assignment.id, "launch_profile_id":profile.id
            })))
            .await
            .unwrap();
        store.claim_launch_job().await.unwrap().unwrap();
        let run = store
            .begin_launch_attempt_with_lsm(attempt.id, Some("shared-runtime"))
            .await
            .unwrap()
            .run;
        let app = routes().with_state(AppState {
            store: store.clone(),
        });
        let (status, response) =
            request(&app, "POST", &format!("/runs/{}/cancel", run.id), json!({})).await;
        assert_eq!(status, StatusCode::OK, "{response}");
        assert_eq!(response["status"], "cancelling");
        assert_eq!(store.get_run(run.id).await.unwrap().status, "cancelling");
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
            default_cwd: Some("/tmp/work".into()),
            model: Some("gpt-test".into()),
            enabled: true,
            metadata: json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let args = codex_args(&profile, "/tmp/work", "/tmp/last", None);
        assert_eq!(args.last().map(String::as_str), Some("-"));
        assert!(!args.join(" ").contains("task"));
        assert!(!args.iter().any(|arg| arg.contains("rm -rf")));
        let resumed = codex_args(&profile, "/tmp/work", "/tmp/last", Some("session-123"));
        assert_eq!(&resumed[0..3], &["exec", "resume", "--json"]);
        assert_eq!(resumed[resumed.len() - 2], "session-123");
        assert!(!resumed.join(" ").contains("task"));
    }

    #[test]
    fn codex_morrows_auth_config_uses_env_names_not_credentials() {
        let profile = LaunchProfile {
            id: uuid::Uuid::new_v4(),
            name: "codex".into(),
            adapter: "codex_cli".into(),
            agent_instance_id: uuid::Uuid::new_v4(),
            program: "/bin/codex".into(),
            default_cwd: Some("/tmp/work".into()),
            model: None,
            enabled: true,
            metadata: json!({}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let mut args = codex_args(&profile, "/tmp/work", "/tmp/last", None);
        inject_morrows_config(&mut args);
        let joined = args.join(" ");
        assert!(joined.contains("MORROWS_AGENT_AUTHORIZATION"));
        assert!(joined.contains("MORROWS_AGENT_INSTANCE_ID"));
        assert!(!joined.contains("Bearer "));
        assert!(!joined.contains("mrw_agent_"));
    }

    #[test]
    fn codebuddy_argv_contains_only_control_metadata_not_prompt_or_capability() {
        let profile = LaunchProfile {
            id: uuid::Uuid::new_v4(),
            name: "codebuddy".into(),
            adapter: "codebuddy_cli".into(),
            agent_instance_id: uuid::Uuid::new_v4(),
            program: "/opt/homebrew/bin/codebuddy".into(),
            default_cwd: Some("/tmp/work".into()),
            model: Some("glm-5.3-flash".into()),
            enabled: true,
            metadata: json!({"permission_mode":"auto"}),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let args = codebuddy_args(
            &profile,
            "/tmp/private-mcp-config.json",
            "morrows-session-1",
            false,
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--session-id", "morrows-session-1"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--mcp-config", "/tmp/private-mcp-config.json"])
        );
        assert!(
            args.iter()
                .any(|arg| arg == "--dangerously-skip-permissions")
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--tools", "ToolSearch,DeferExecuteTool"])
        );
        assert!(!args.join(" ").contains("task body"));
        assert!(!args.join(" ").contains("LSM-CAPABILITY"));
        assert!(!args.join(" ").contains("X-LSM-Session-Capability"));

        let resumed = codebuddy_args(
            &profile,
            "/tmp/private-mcp-config.json",
            "morrows-session-1",
            true,
        );
        assert!(
            resumed
                .windows(2)
                .any(|pair| pair == ["--resume", "morrows-session-1"])
        );
        assert!(!resumed.iter().any(|arg| arg == "--session-id"));
    }

    #[tokio::test]
    async fn fake_codebuddy_process_receives_delivery_and_persists_resumable_session() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let program = fake_executable(0);
        let cwd = program.parent().unwrap().to_path_buf();
        let agent = make_agent(&store).await;
        let task = store
            .create_task(input(json!({
                "title":"CodeBuddy fake runtime",
                "description":"No real provider call"
            })))
            .await
            .unwrap();
        let assignment = store
            .claim_task(task.id, agent.id, "executor", 600)
            .await
            .unwrap();
        let profile = store
            .register_launch_profile(input(json!({
                "name":"fake codebuddy",
                "adapter":"codebuddy_cli",
                "agent_instance_id":agent.id,
                "program":program,
                "default_cwd":cwd,
                "model":"glm-5.3-flash",
                "enabled":true
            })))
            .await
            .unwrap();
        let general_session = store
            .create_session(CreateSession {
                agent_instance_id: agent.id,
                title: "Unscoped discussion".into(),
            })
            .await
            .unwrap();
        store
            .create_human_session_message(general_session.id, "do not inject into task")
            .await
            .unwrap();

        let attempt = store
            .enqueue_launch(input(json!({
                "assignment_id":assignment.id,
                "launch_profile_id":profile.id
            })))
            .await
            .unwrap();
        let task_session_id = attempt
            .session_id
            .expect("task launch should bind a Session");
        let task_message = store
            .create_human_session_message(task_session_id, "delivery to codebuddy")
            .await
            .unwrap();
        let deliveries = store.agent_delivery_inbox(agent.id, 80).await.unwrap();
        let task_delivery_id = deliveries
            .iter()
            .find(|delivery| delivery.source_id == task_message.id)
            .unwrap()
            .id;
        let general_delivery_id = deliveries
            .iter()
            .find(|delivery| delivery.payload["session_id"] == general_session.id.to_string())
            .unwrap()
            .id;

        store.claim_launch_job().await.unwrap().unwrap();
        let execution = store.begin_launch_attempt(attempt.id).await.unwrap();
        let root = cwd.join("codebuddy-launches");
        execute_codebuddy_with_root(store.clone(), execution, root.clone())
            .await
            .unwrap();

        let finished = store.get_launch_attempt(attempt.id).await.unwrap();
        assert_eq!(finished.status, "completed");
        assert_eq!(
            finished.external_session_ref.as_deref(),
            Some(format!("morrows-{}", attempt.id).as_str())
        );
        assert_eq!(
            store
                .latest_session_provider_ref(task_session_id, profile.id)
                .await
                .unwrap()
                .as_deref(),
            Some(format!("morrows-{}", attempt.id).as_str()),
            "Task launches and direct Session runtimes must share Provider thread continuity"
        );
        let delivered = store.get_agent_delivery(task_delivery_id).await.unwrap();
        assert_eq!(delivered.status, "delivered");
        assert!(
            delivered
                .delivered_by
                .as_deref()
                .is_some_and(|value| value.starts_with("codebuddy_cli:"))
        );
        let general = store.get_agent_delivery(general_delivery_id).await.unwrap();
        assert_eq!(
            general.status, "queued",
            "an unscoped Session message must not leak into a Task launch"
        );
        assert!(
            !root
                .join(attempt.id.to_string())
                .join("codebuddy-mcp.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn delivery_prompt_includes_direct_message_and_avoids_duplicate_current_instruction() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let program = fake_executable(0);
        let cwd = program.parent().unwrap().to_path_buf();
        let (attempt, agent) = make_execution(&store, &program, &cwd).await;
        store
            .send_launch_instruction(
                SendLaunchInstruction {
                    launch_attempt_id: attempt.id,
                    body: "instruction already loaded by launch execution".into(),
                },
                None,
            )
            .await
            .unwrap();
        let session = store
            .create_session(CreateSession {
                agent_instance_id: agent.id,
                title: "Prompt delivery".into(),
            })
            .await
            .unwrap();
        store
            .create_human_session_message(session.id, "answer this direct message")
            .await
            .unwrap();

        store.claim_launch_job().await.unwrap().unwrap();
        let mut execution = store.begin_launch_attempt(attempt.id).await.unwrap();
        execution.context = Some(store.create_context_revision(execution.task.id, serde_json::from_value(json!({
            "background":"Machine: test-node; checkout: /srv/work/audit; original report: work/report.json"
        })).unwrap()).await.unwrap());
        let deliveries = store.agent_delivery_inbox(agent.id, 80).await.unwrap();
        let delivery_prompt = build_delivery_prompt(&execution, &deliveries);
        assert!(delivery_prompt.contains("answer this direct message"));
        assert!(delivery_prompt.contains(&session.id.to_string()));
        assert!(!delivery_prompt.contains("instruction already loaded by launch execution"));
        assert!(
            build_prompt(&execution).contains("instruction already loaded by launch execution")
        );
        let prompt = build_prompt(&execution);
        assert!(prompt.contains("checkout: /srv/work/audit; original report: work/report.json"));
        assert_eq!(
            prompt.matches("Context preparation and capture:").count(),
            1
        );
        assert!(prompt.contains("observation time, verification status"));
        assert_eq!(
            prompt
                .matches("Execution persistence and completion:")
                .count(),
            1
        );
        assert!(prompt.contains("project_memory_publish"));
        assert!(prompt.contains("run_completion_check"));
        assert!(prompt.contains(include_str!("context_capture_instructions.md")));
        assert!(prompt.contains(include_str!("execution_workflow_instructions.md")));
    }

    #[tokio::test]
    async fn external_accept_requires_assigned_agent_header_and_instructions_are_visible() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = make_agent(&store).await;
        let task = store
            .create_task(input(json!({"title":"External REST"})))
            .await
            .unwrap();
        let assignment = store
            .claim_task(task.id, agent.id, "executor", 600)
            .await
            .unwrap();
        let profile = store
            .register_launch_profile(input(json!({
                "name":"LSM", "adapter":"lsm_external", "agent_instance_id":agent.id
            })))
            .await
            .unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });
        let (status, attempt) = request(
            &app,
            "POST",
            "/launch-attempts/enqueue",
            json!({
                "assignment_id":assignment.id, "launch_profile_id":profile.id
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(attempt["status"], "awaiting_agent");
        let id = attempt["id"].as_str().unwrap();
        assert_eq!(
            request(
                &app,
                "POST",
                "/launch-attempts/external/accept",
                json!({
                    "launch_attempt_id":id,"external_session_ref":"lsm-session"
                })
            )
            .await
            .0,
            StatusCode::BAD_REQUEST
        );
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/launch-attempts/external/accept")
                    .header("content-type", "application/json")
                    .header("X-Agent-Instance-Id", agent.id.to_string())
                    .body(Body::from(
                        json!({"launch_attempt_id":id,"external_session_ref":"lsm-session"})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let (status, instruction) = request(
            &app,
            "POST",
            &format!("/launch-attempts/{id}/instructions"),
            json!({
                "launch_attempt_id":id,"body":"Use the saved context"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(instruction["body"], "Use the saved context");
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("/tasks/{}/launch-instructions", task.id),
                json!(null)
            )
            .await
            .1
            .as_array()
            .unwrap()
            .len(),
            1
        );
    }

    #[tokio::test]
    async fn stopping_a_running_local_launcher_kills_its_child() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let program = fake_executable(0);
        std::fs::write(&program, "#!/bin/sh\ncat >/dev/null\nexec sleep 30\n").unwrap();
        let cwd = program.parent().unwrap().to_path_buf();
        let (attempt, _) = make_execution(&store, &program, &cwd).await;
        let job = store.claim_launch_job().await.unwrap().unwrap();
        let execution = store.begin_launch_attempt(job.attempt.id).await.unwrap();
        let worker_store = store.clone();
        let root = cwd.join("launches");
        let handle =
            tokio::spawn(
                async move { execute_codex_with_root(worker_store, execution, root).await },
            );
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if store.get_launch_attempt(attempt.id).await.unwrap().status == "running" {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        store.stop_launch_attempt(attempt.id).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), handle)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            store.get_launch_attempt(attempt.id).await.unwrap().status,
            "cancelled"
        );
        assert_eq!(
            store
                .get_assignment(attempt.assignment_id)
                .await
                .unwrap()
                .status,
            "released"
        );
        let _ = std::fs::remove_dir_all(cwd);
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
