use super::*;
use axum::extract::Query;
use morrows_core::CreateMemoryEntry;

#[derive(Debug, Deserialize)]
struct MemoryQuery {
    scope_type: Option<String>,
    project_id: Option<Id>,
    agent_instance_id: Option<Id>,
    task_id: Option<Id>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/memories", get(list_memories).post(create_memory))
        .route("/memories/{id}", get(get_memory))
        .route(
            "/projects/{id}/memory/mirror",
            axum::routing::post(mirror_memory),
        )
        .route(
            "/projects/{id}/memory/cutover",
            axum::routing::post(cutover_memory),
        )
        .route("/memory/reconcile", axum::routing::post(reconcile_memory))
        .route(
            "/projects/{id}/memory/rebuild",
            axum::routing::post(rebuild_memory),
        )
}

async fn list_memories(
    State(state): State<AppState>,
    Query(query): Query<MemoryQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .list_memory_entries(
                query.scope_type.as_deref(),
                query.project_id,
                query.agent_instance_id,
                query.task_id,
            )
            .await?
    )))
}

async fn create_memory(
    State(state): State<AppState>,
    Json(input): Json<CreateMemoryEntry>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(state.store.create_memory_entry(input).await?)))
}

async fn get_memory(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(state.store.get_memory_entry(id).await?)))
}

// These are operator control-plane routes, never employee MCP mutations.
async fn mirror_memory(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    let commit = state.store.mirror_project_memory(id).await?;
    Ok(Json(
        json!({"project_id":id,"verified_commit":commit,"backend":"sql"}),
    ))
}
#[derive(Deserialize)]
struct CutoverMemory {
    verified_commit: String,
}
async fn cutover_memory(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<CutoverMemory>,
) -> Result<Json<Value>, ApiError> {
    state
        .store
        .cutover_project_memory(id, &input.verified_commit)
        .await?;
    Ok(Json(
        json!({"project_id":id,"memory_head":state.store.project_memory_head(id).await?,"backend":"git"}),
    ))
}
async fn reconcile_memory(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    state.store.reconcile_git_memory().await?;
    Ok(Json(json!({"reconciled":true})))
}

async fn rebuild_memory(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    state.store.rebuild_project_memory_projection(id).await?;
    Ok(Json(json!({"project_id":id,"rebuilt":true})))
}
