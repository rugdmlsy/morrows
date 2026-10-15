use super::*;
use axum::extract::Query;
use morrows_core::CreateMemoryEntry;

#[derive(Debug, Deserialize)]
struct ProjectMemoriesQuery {
    limit: Option<i64>,
    offset: Option<i64>,
}

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
        .route(
            "/projects/{id}/memories",
            get(list_current_project_memories),
        )
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

/// The WebUI reads only the current, shared memories for this project,
/// bounded so large projects do not block the detail panel.
async fn list_current_project_memories(
    State(state): State<AppState>,
    Path(id): Path<Id>,
    Query(query): Query<ProjectMemoriesQuery>,
) -> Result<Json<Value>, ApiError> {
    state.store.get_project(id).await?;
    let page = state
        .store
        .project_memories_page(id, query.limit.unwrap_or(30), query.offset.unwrap_or(0))
        .await?;
    Ok(Json(json!(page)))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn project_memories_endpoint_returns_paged_current_memory() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let project = store
            .create_project(serde_json::from_value(json!({"name": "UI project"})).unwrap())
            .await
            .unwrap();
        let old = store
            .create_memory_entry(
                serde_json::from_value(json!({
                    "scope_type": "project", "project_id": project.id, "title": "old",
                    "content": "old", "source_kind": "test"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .create_memory_entry(
                serde_json::from_value(json!({
                    "scope_type": "project", "project_id": project.id, "title": "current",
                    "content": "current", "source_kind": "test", "supersedes_memory_id": old.id
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let Json(payload) = list_current_project_memories(
            State(AppState { store }),
            Path(project.id),
            Query(ProjectMemoriesQuery {
                limit: Some(30),
                offset: Some(0),
            }),
        )
        .await
        .unwrap();
        assert_eq!(payload["items"].as_array().unwrap().len(), 1);
        assert_eq!(payload["items"][0]["title"], "current");
        assert!(payload["next_offset"].is_null());
    }
}
