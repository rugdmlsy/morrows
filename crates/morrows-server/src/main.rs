mod assignment_request;
mod auth;
mod collaboration;
mod delivery;
mod dispatch;
mod fleet;
mod launch;
mod lsm;
mod mcp;
mod memory;
mod memory_search;
mod operator_auth;
mod session;

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mcp::MorrowsMcp;
use memory_search::MemorySearch;
use morrows_core::{CreateContextRevision, CreateProject, CreateTask, DomainError, Id};
use morrows_store::Store;
use rmcp::transport::{
    StreamableHttpServerConfig,
    streamable_http_server::{session::local::LocalSessionManager, tower::StreamableHttpService},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{env, net::SocketAddr};
use tower_http::{cors::CorsLayer, services::ServeDir, trace::TraceLayer};
use tracing::info;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    store: Store,
}

/// Managed builds place the employee CLI next to the server. Expose its path to
/// child runtimes without changing their PATH or assuming the agent's cwd.
fn configure_memory_cli(command: &mut tokio::process::Command) {
    command.env_remove("MORROWS_MEMORY_CLI");
    if let Ok(executable) = std::env::current_exe()
        && let Some(parent) = executable.parent()
    {
        let cli = parent.join(if cfg!(windows) {
            "morrows.exe"
        } else {
            "morrows"
        });
        if cli.is_file() {
            command.env("MORROWS_MEMORY_CLI", cli);
        }
    }
}

#[derive(Debug)]
struct ApiError(DomainError);

impl From<DomainError> for ApiError {
    fn from(value: DomainError) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            DomainError::NotFound(_) => StatusCode::NOT_FOUND,
            DomainError::Conflict(_) => StatusCode::CONFLICT,
            DomainError::InvalidState(_) | DomainError::InvalidInput(_) => StatusCode::BAD_REQUEST,
            DomainError::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"error": self.0.to_string()}))).into_response()
    }
}

#[derive(Deserialize)]
struct RegisterAgentBody {
    name: String,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Deserialize)]
struct SetTaskProjectBody {
    project_id: Option<Id>,
}

#[derive(Deserialize)]
struct ClaimBody {
    agent_instance_id: Id,
    #[serde(default = "default_role")]
    role: String,
    #[serde(default = "default_lease")]
    lease_seconds: i64,
}
fn default_role() -> String {
    "executor".into()
}
fn default_lease() -> i64 {
    900
}

#[derive(Deserialize)]
struct StartRunBody {
    assignment_id: Id,
    agent_instance_id: Id,
    external_session_ref: Option<String>,
}

#[derive(Deserialize)]
struct RunMutationBody {
    agent_instance_id: Id,
    #[serde(default)]
    payload: Value,
}

#[derive(Deserialize)]
struct RunMilestoneBody {
    agent_instance_id: Id,
    input: morrows_core::CreateRunMilestone,
}

#[derive(Deserialize)]
struct RenewAssignmentBody {
    agent_instance_id: Id,
    #[serde(default = "default_lease")]
    lease_seconds: i64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "morrows_server=info,tower_http=info".into()),
        )
        .init();

    let db_url = env::var("MORROWS_DATABASE_URL")
        .or_else(|_| env::var("AC_DATABASE_URL"))
        .unwrap_or_else(|_| "sqlite://data/morrows.db".into());
    let bind = env::var("MORROWS_BIND")
        .or_else(|_| env::var("AC_BIND"))
        .unwrap_or_else(|_| "127.0.0.1:8787".into());
    let addr: SocketAddr = bind.parse().context("parse MORROWS_BIND")?;
    let require_agent_auth = env_bool("MORROWS_REQUIRE_AGENT_AUTH", false)?;
    let require_operator_auth = env_bool("MORROWS_REQUIRE_OPERATOR_AUTH", false)?;
    let allow_insecure_remote_http = env_bool("MORROWS_ALLOW_INSECURE_REMOTE_HTTP", false)?;
    let bootstrap_operator_token = env::var("MORROWS_BOOTSTRAP_OPERATOR_TOKEN")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if bootstrap_operator_token
        .as_deref()
        .is_some_and(|value| !value.starts_with("mrw_operator_"))
    {
        anyhow::bail!("MORROWS_BOOTSTRAP_OPERATOR_TOKEN must start with mrw_operator_");
    }

    let store = Store::connect(&db_url)
        .await
        .context("open Morrows database")?;
    let has_admin_operator = store.has_active_admin_operator_credential().await?;
    validate_bind_security(
        addr,
        require_agent_auth,
        require_operator_auth,
        bootstrap_operator_token.is_some(),
        has_admin_operator,
        allow_insecure_remote_http,
    )?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let recovered = store.recover_launch_jobs_after_restart().await?;
    if recovered > 0 {
        tracing::warn!(recovered, "reconciled interrupted launch jobs");
    }
    let recovered_session_runtimes = store
        .recover_session_runtime_attempts_after_restart()
        .await?;
    if recovered_session_runtimes > 0 {
        tracing::warn!(
            recovered_session_runtimes,
            "marked interrupted Session runtimes failed after restart"
        );
    }
    let recovered_deliveries = store.recover_agent_delivery_claims().await?;
    if recovered_deliveries > 0 {
        tracing::warn!(
            recovered_deliveries,
            "returned abandoned Agent delivery claims to the queue"
        );
    }
    let managed_memory_search = MemorySearch::managed();
    tracing::info!(
        enabled = managed_memory_search.is_some(),
        "configured ripgrep project-memory retrieval"
    );
    let state = AppState {
        store: store.clone(),
    };

    let api = Router::new()
        .route("/health", get(health))
        .route("/projects", get(list_projects).post(create_project))
        .route("/projects/{id}", get(get_project))
        .route("/tasks", get(list_tasks).post(create_task))
        .route("/tasks/{id}", get(get_task))
        .route("/tasks/{id}/project", post(set_task_project))
        .route("/tasks/{id}/claim", post(claim_task))
        .route("/assignments/{id}/renew", post(renew_assignment))
        .route("/tasks/{id}/context", get(get_context).post(create_context))
        .route("/tasks/{id}/events", get(task_events))
        .route("/tasks/{id}/assignments", get(task_assignments))
        .route("/tasks/{id}/runs", get(task_runs))
        .route("/agents", get(list_agents).post(register_agent))
        .route("/runs", post(start_run))
        .route("/runs/{id}/checkpoint", post(checkpoint_run))
        .route("/runs/{id}/milestones", post(create_run_milestone))
        .route("/runs/{id}/complete", post(complete_run))
        .merge(auth::routes())
        .merge(memory::routes())
        .merge(assignment_request::routes())
        .merge(operator_auth::routes())
        .merge(collaboration::routes())
        .merge(session::routes())
        .merge(dispatch::routes())
        .merge(delivery::routes())
        .merge(fleet::routes())
        .merge(launch::routes())
        .with_state(state);

    let mcp_public_url = env::var("MORROWS_MCP_URL").ok();
    let mcp_extra_hosts = env::var("MORROWS_MCP_ALLOWED_HOSTS").ok();
    let mcp_allowed_hosts =
        build_mcp_allowed_hosts(mcp_public_url.as_deref(), mcp_extra_hosts.as_deref());
    tracing::info!(?mcp_allowed_hosts, "configured Morrows MCP Host allowlist");

    let mcp_store = store.clone();
    let mcp_service: StreamableHttpService<MorrowsMcp, LocalSessionManager> =
        StreamableHttpService::new(
            move || {
                Ok(MorrowsMcp::new_with_memory_search(
                    mcp_store.clone(),
                    managed_memory_search.clone(),
                ))
            },
            Default::default(),
            StreamableHttpServerConfig::default()
                .with_json_response(true)
                .with_allowed_hosts(mcp_allowed_hosts),
        );

    let lease_store = store.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            match lease_store.expire_stale_assignments().await {
                Ok(n) if n > 0 => tracing::info!(expired = n, "expired stale assignments"),
                Ok(_) => {}
                Err(err) => tracing::error!(%err, "lease expiry scan failed"),
            }
        }
    });

    let dispatch_store = store.clone();
    tokio::spawn(async move {
        dispatch::scheduler_loop(dispatch_store).await;
    });
    let launch_store = store.clone();
    tokio::spawn(async move {
        launch::worker_loop(launch_store).await;
    });
    let delivery_store = store.clone();
    tokio::spawn(async move {
        launch::delivery_worker_loop(delivery_store).await;
    });
    let runtime_store = store.clone();
    tokio::spawn(async move {
        launch::runtime_sweep(runtime_store).await;
    });

    let web_dir = env::var("MORROWS_WEB_DIR")
        .or_else(|_| env::var("AC_WEB_DIR"))
        .unwrap_or_else(|_| "web/dist".into());
    let auth_state = auth::AgentAuthState::new(store.clone(), require_agent_auth);
    let operator_auth_state = operator_auth::OperatorAuthState::new(
        store.clone(),
        require_operator_auth,
        bootstrap_operator_token,
    );
    let app = Router::new()
        .nest("/api", api)
        .nest_service("/mcp", mcp_service)
        .fallback_service(ServeDir::new(web_dir).append_index_html_on_directories(true))
        .layer(middleware::from_fn_with_state(
            auth_state,
            auth::authenticate_agent_requests,
        ))
        .layer(middleware::from_fn_with_state(
            operator_auth_state,
            operator_auth::authenticate_operator_requests,
        ));
    let app = if addr.ip().is_loopback() {
        app.layer(CorsLayer::permissive())
    } else {
        app
    };
    let app = app.layer(TraceLayer::new_for_http());

    info!(%addr, "Morrows server listening");
    axum::serve(listener, app).await?;
    Ok(())
}

fn validate_bind_security(
    addr: SocketAddr,
    require_agent_auth: bool,
    require_operator_auth: bool,
    has_bootstrap_operator: bool,
    has_admin_operator: bool,
    allow_insecure_remote_http: bool,
) -> anyhow::Result<()> {
    if require_operator_auth && !has_bootstrap_operator && !has_admin_operator {
        anyhow::bail!(
            "MORROWS_REQUIRE_OPERATOR_AUTH=1 requires MORROWS_BOOTSTRAP_OPERATOR_TOKEN or an active durable admin operator credential"
        );
    }
    if !addr.ip().is_loopback() {
        if !require_operator_auth {
            anyhow::bail!(
                "non-loopback MORROWS_BIND={addr} requires MORROWS_REQUIRE_OPERATOR_AUTH=1"
            );
        }
        if !require_agent_auth {
            anyhow::bail!("non-loopback MORROWS_BIND={addr} requires MORROWS_REQUIRE_AGENT_AUTH=1");
        }
        if !allow_insecure_remote_http {
            anyhow::bail!(
                "Morrows refuses direct non-loopback HTTP because Bearer credentials require transport security; keep MORROWS_BIND on loopback and expose it through a TLS reverse proxy, or set MORROWS_ALLOW_INSECURE_REMOTE_HTTP=1 only for isolated development"
            );
        }
    }
    Ok(())
}

fn build_mcp_allowed_hosts(public_url: Option<&str>, configured: Option<&str>) -> Vec<String> {
    let mut hosts = vec![
        "localhost".to_owned(),
        "127.0.0.1".to_owned(),
        "::1".to_owned(),
    ];
    let mut push = |value: &str| {
        let value = value.trim();
        if !value.is_empty() && !hosts.iter().any(|host| host == value) {
            hosts.push(value.to_owned());
        }
    };

    if let Some(public_url) = public_url
        && let Some((_, rest)) = public_url.trim().split_once("://")
        && let Some(authority) = rest.split('/').next()
    {
        push(authority);
    }
    if let Some(configured) = configured {
        for host in configured.split(',') {
            push(host);
        }
    }
    hosts
}

fn env_bool(name: &str, default: bool) -> anyhow::Result<bool> {
    let Ok(value) = env::var(name) else {
        return Ok(default);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => anyhow::bail!("{name} must be a boolean"),
    }
}

async fn health() -> Json<Value> {
    Json(json!({
        "ok": true,
        "service": "morrows",
        "mcp": {"ready": true, "path": "/mcp"},
        "memory_search": {
            "enabled": MemorySearch::managed().is_some(),
            "engine": "ripgrep",
            "index_role": "none"
        }
    }))
}

async fn list_projects(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.list_projects().await?).unwrap(),
    ))
}

async fn create_project(
    State(state): State<AppState>,
    Json(body): Json<CreateProject>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.create_project(body).await?).unwrap(),
    ))
}

async fn get_project(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.get_project(id).await?).unwrap(),
    ))
}

async fn list_tasks(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.list_tasks().await?).unwrap(),
    ))
}

async fn create_task(
    State(state): State<AppState>,
    Json(body): Json<CreateTask>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.create_task(body).await?).unwrap(),
    ))
}

async fn get_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.get_task(id).await?).unwrap(),
    ))
}

async fn set_task_project(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<SetTaskProjectBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.set_task_project(id, body.project_id).await?).unwrap(),
    ))
}

async fn register_agent(
    State(state): State<AppState>,
    Json(body): Json<RegisterAgentBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(
            state
                .store
                .register_agent(&body.name, &body.capabilities)
                .await?,
        )
        .unwrap(),
    ))
}

async fn list_agents(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.list_agents().await?).unwrap(),
    ))
}

async fn claim_task(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<ClaimBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(
            state
                .store
                .claim_task(id, body.agent_instance_id, &body.role, body.lease_seconds)
                .await?,
        )
        .unwrap(),
    ))
}

async fn renew_assignment(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<RenewAssignmentBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(
            state
                .store
                .renew_assignment(id, body.agent_instance_id, body.lease_seconds)
                .await?,
        )
        .unwrap(),
    ))
}

async fn start_run(
    State(state): State<AppState>,
    Json(body): Json<StartRunBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(
            state
                .store
                .start_run(
                    body.assignment_id,
                    body.agent_instance_id,
                    body.external_session_ref,
                )
                .await?,
        )
        .unwrap(),
    ))
}

async fn checkpoint_run(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<RunMutationBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(
            state
                .store
                .checkpoint_run(id, body.agent_instance_id, body.payload)
                .await?,
        )
        .unwrap(),
    ))
}

async fn create_run_milestone(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<RunMilestoneBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(
            state
                .store
                .create_run_milestone(id, body.agent_instance_id, body.input)
                .await?,
        )
        .unwrap(),
    ))
}

async fn complete_run(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<RunMutationBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(
            state
                .store
                .complete_run(id, body.agent_instance_id, body.payload)
                .await?,
        )
        .unwrap(),
    ))
}

async fn get_context(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.get_current_context(id).await?).unwrap(),
    ))
}

async fn create_context(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(body): Json<CreateContextRevision>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.create_context_revision(id, body).await?).unwrap(),
    ))
}

async fn task_assignments(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.task_assignments(id).await?).unwrap(),
    ))
}

async fn task_runs(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.task_runs(id).await?).unwrap(),
    ))
}

async fn task_events(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(
        serde_json::to_value(state.store.task_events(id).await?).unwrap(),
    ))
}

#[cfg(test)]
mod startup_security_tests {
    use super::*;

    fn addr(value: &str) -> SocketAddr {
        value.parse().unwrap()
    }

    #[test]
    fn mcp_host_allowlist_keeps_loopback_and_adds_public_and_explicit_hosts() {
        assert_eq!(
            build_mcp_allowed_hosts(
                Some("https://mcp.xycdev.com/morrows"),
                Some("internal.example:9443, mcp.xycdev.com"),
            ),
            vec![
                "localhost",
                "127.0.0.1",
                "::1",
                "mcp.xycdev.com",
                "internal.example:9443",
            ]
        );
        assert_eq!(
            build_mcp_allowed_hosts(Some("not-a-url"), Some("")),
            vec!["localhost", "127.0.0.1", "::1"]
        );
    }

    #[test]
    fn loopback_compatibility_remains_available() {
        validate_bind_security(addr("127.0.0.1:8787"), false, false, false, false, false).unwrap();
    }

    #[test]
    fn strict_operator_auth_cannot_start_without_bootstrap_or_admin() {
        let err = validate_bind_security(addr("127.0.0.1:8787"), true, true, false, false, false)
            .unwrap_err();
        assert!(err.to_string().contains("MORROWS_BOOTSTRAP_OPERATOR_TOKEN"));

        validate_bind_security(addr("127.0.0.1:8787"), true, true, false, true, false).unwrap();
    }

    #[test]
    fn non_loopback_requires_both_auth_planes_and_explicit_plain_http_override() {
        let remote = addr("0.0.0.0:8787");

        assert!(
            validate_bind_security(remote, true, false, true, false, false)
                .unwrap_err()
                .to_string()
                .contains("MORROWS_REQUIRE_OPERATOR_AUTH")
        );
        assert!(
            validate_bind_security(remote, false, true, true, false, false)
                .unwrap_err()
                .to_string()
                .contains("MORROWS_REQUIRE_AGENT_AUTH")
        );
        assert!(
            validate_bind_security(remote, true, true, true, false, false)
                .unwrap_err()
                .to_string()
                .contains("refuses direct non-loopback HTTP")
        );
        validate_bind_security(remote, true, true, true, false, true).unwrap();
    }
}
