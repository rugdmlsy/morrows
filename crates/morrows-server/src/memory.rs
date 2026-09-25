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
