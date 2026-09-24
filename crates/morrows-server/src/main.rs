mod auth;
mod collaboration;
mod conversation;
mod delivery;
mod dispatch;
mod fleet;
mod launch;
mod lsm;
mod mcp;

use anyhow::Context;
use axum::{
    Json, Router,
    extract::{Path, State},
    middleware,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use mcp::MorrowsMcp;
use morrows_core::{CreateContextRevision, CreateTask, DomainError, Id};
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
    if !addr.ip().is_loopback() {
        anyhow::bail!(
            "Morrows control plane is local-only and refuses non-loopback MORROWS_BIND={addr}; operator authentication is not implemented yet"
        );
    }
    let require_agent_auth = env_bool("MORROWS_REQUIRE_AGENT_AUTH", false)?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let store = Store::connect(&db_url)
        .await
        .context("open Morrows database")?;
    let recovered = store.recover_launch_jobs_after_restart().await?;
    if recovered > 0 {
        tracing::warn!(recovered, "reconciled interrupted launch jobs");
    }
    let recovered_deliveries = store.recover_agent_delivery_claims().await?;
    if recovered_deliveries > 0 {
        tracing::warn!(
            recovered_deliveries,
            "returned abandoned Agent delivery claims to the queue"
        );
    }
    let state = AppState {
        store: store.clone(),
    };

    let api = Router::new()
        .route("/health", get(health))
        .route("/tasks", get(list_tasks).post(create_task))
        .route("/tasks/{id}", get(get_task))
        .route("/tasks/{id}/claim", post(claim_task))
        .route("/assignments/{id}/renew", post(renew_assignment))
        .route("/tasks/{id}/context", get(get_context).post(create_context))
        .route("/tasks/{id}/events", get(task_events))
        .route("/tasks/{id}/assignments", get(task_assignments))
        .route("/tasks/{id}/runs", get(task_runs))
        .route("/agents", get(list_agents).post(register_agent))
        .route("/runs", post(start_run))
        .route("/runs/{id}/checkpoint", post(checkpoint_run))
        .route("/runs/{id}/complete", post(complete_run))
        .merge(auth::routes())
        .merge(collaboration::routes())
        .merge(conversation::routes())
        .merge(dispatch::routes())
        .merge(delivery::routes())
        .merge(fleet::routes())
        .merge(launch::routes())
        .with_state(state);

    let mcp_store = store.clone();
    let mcp_service: StreamableHttpService<MorrowsMcp, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(MorrowsMcp::new(mcp_store.clone())),
            Default::default(),
            StreamableHttpServerConfig::default().with_json_response(true),
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
    let app = Router::new()
        .nest("/api", api)
        .nest_service("/mcp", mcp_service)
        .fallback_service(ServeDir::new(web_dir).append_index_html_on_directories(true))
        .layer(middleware::from_fn_with_state(
            auth_state,
            auth::authenticate_agent_requests,
        ))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    info!(%addr, "Morrows server listening");
    axum::serve(listener, app).await?;
    Ok(())
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
        "mcp": {"ready": true, "path": "/mcp"}
    }))
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
