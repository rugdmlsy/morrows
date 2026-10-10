use super::*;
use morrows_core::TaskRevisionDraft;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tasks/{id}/revisions", get(history).post(propose))
        .route("/tasks/{id}/revisions/preview", post(preview))
        .route("/tasks/{id}/revisions/{revision_id}/reject", post(reject))
}
async fn history(State(s): State<AppState>, Path(id): Path<Id>) -> Result<Json<Value>, ApiError> {
    Ok(Json(s.store.task_revision_history(id).await?))
}
async fn preview(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<TaskRevisionDraft>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(s.store.preview_task_revision(id, input).await?))
}
async fn propose(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    axum::Extension(identity): axum::Extension<operator_auth::OperatorIdentity>,
    Json(input): Json<TaskRevisionDraft>,
) -> Result<Json<Value>, ApiError> {
    // A browser must authenticate to the operator control plane; an Agent Bearer cannot
    // forge a Human revision. Employee MCP separately binds author to the authenticated Agent.
    let actor = if identity.0["authenticated"].as_bool() == Some(false) {
        "human:local".to_owned()
    } else {
        format!(
            "operator:{}",
            identity.0["label"].as_str().unwrap_or("authenticated")
        )
    };
    Ok(Json(
        s.store.propose_task_revision(id, &actor, input).await?,
    ))
}
#[derive(Deserialize)]
struct RejectRequest {
    reason: String,
}
async fn reject(
    State(s): State<AppState>,
    Path((id, revision_id)): Path<(Id, Id)>,
    axum::Extension(identity): axum::Extension<operator_auth::OperatorIdentity>,
    Json(body): Json<RejectRequest>,
) -> Result<Json<Value>, ApiError> {
    let actor = if identity.0["authenticated"].as_bool() == Some(false) {
        "human:local".to_owned()
    } else {
        format!(
            "operator:{}",
            identity.0["label"].as_str().unwrap_or("authenticated")
        )
    };
    Ok(Json(
        s.store
            .reject_task_revision(id, &actor, revision_id, &body.reason)
            .await?,
    ))
}
