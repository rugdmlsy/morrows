use super::*;
use morrows_core::*;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/task-chains", get(chains).post(create_chain))
        .route(
            "/task-chains/{id}",
            get(chain).put(update_chain).delete(archive_chain),
        )
        .route("/task-chains/{id}/archive", post(archive_chain))
        .route("/task-chains/{id}/graph", get(chain))
        .route(
            "/task-chains/{id}/members/{task}",
            axum::routing::put(chain_member).delete(chain_member_remove),
        )
        .route("/task-chains/{id}/edges", post(edge))
        .route(
            "/task-chains/{id}/edges/{task}/{predecessor}",
            axum::routing::delete(edge_remove),
        )
        .route("/task-groups", get(groups).post(create_group))
        .route(
            "/task-groups/{id}",
            get(group).put(update_group).delete(archive_group),
        )
        .route("/task-groups/{id}/archive", post(archive_group))
        .route(
            "/task-groups/{id}/members/{task}",
            axum::routing::put(group_member).delete(group_member_remove),
        )
        .route("/task-groups/batch-assign", post(batch_assign))
        .route("/tasks/{id}/gate", get(gate))
}
async fn chains(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.list_task_collections(true).await?)))
}
async fn groups(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.list_task_collections(false).await?)))
}
async fn chain(State(s): State<AppState>, Path(id): Path<Id>) -> Result<Json<Value>, ApiError> {
    Ok(Json(s.store.task_collection_detail(true, id).await?))
}
async fn group(State(s): State<AppState>, Path(id): Path<Id>) -> Result<Json<Value>, ApiError> {
    Ok(Json(s.store.task_collection_detail(false, id).await?))
}
async fn create_chain(
    State(s): State<AppState>,
    Json(input): Json<SaveTaskCollection>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.save_task_collection(true, None, input).await?
    )))
}
async fn create_group(
    State(s): State<AppState>,
    Json(input): Json<SaveTaskCollection>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.save_task_collection(false, None, input).await?
    )))
}
async fn update_chain(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<SaveTaskCollection>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.save_task_collection(true, Some(id), input).await?
    )))
}
async fn update_group(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<SaveTaskCollection>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.save_task_collection(false, Some(id), input).await?
    )))
}
async fn archive_chain(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    s.store.archive_task_collection(true, id).await?;
    Ok(Json(json!({"archived":true})))
}
async fn archive_group(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    s.store.archive_task_collection(false, id).await?;
    Ok(Json(json!({"archived":true})))
}
#[derive(Deserialize)]
struct MemberInput {
    #[serde(default)]
    join_mode: JoinMode,
}
async fn chain_member(
    State(s): State<AppState>,
    Path((id, task)): Path<(Id, Id)>,
    Json(input): Json<MemberInput>,
) -> Result<Json<Value>, ApiError> {
    s.store
        .set_collection_member(true, id, task, input.join_mode, false)
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn group_member(
    State(s): State<AppState>,
    Path((id, task)): Path<(Id, Id)>,
) -> Result<Json<Value>, ApiError> {
    s.store
        .set_collection_member(false, id, task, JoinMode::All, false)
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn chain_member_remove(
    State(s): State<AppState>,
    Path((id, task)): Path<(Id, Id)>,
) -> Result<Json<Value>, ApiError> {
    s.store
        .set_collection_member(true, id, task, JoinMode::All, true)
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn group_member_remove(
    State(s): State<AppState>,
    Path((id, task)): Path<(Id, Id)>,
) -> Result<Json<Value>, ApiError> {
    s.store
        .set_collection_member(false, id, task, JoinMode::All, true)
        .await?;
    Ok(Json(json!({"ok":true})))
}
async fn edge(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<SaveChainEdge>,
) -> Result<Json<Value>, ApiError> {
    s.store.save_chain_edge(id, input).await?;
    Ok(Json(json!({"ok":true})))
}
async fn edge_remove(
    State(s): State<AppState>,
    Path((id, task, predecessor)): Path<(Id, Id, Id)>,
) -> Result<Json<Value>, ApiError> {
    s.store.remove_chain_edge(id, task, predecessor).await?;
    Ok(Json(json!({"ok":true})))
}
async fn batch_assign(
    State(s): State<AppState>,
    Json(input): Json<GroupBatchAssign>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.assign_task_group(input).await?)))
}
async fn gate(State(s): State<AppState>, Path(id): Path<Id>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.task_gate(id).await?)))
}
