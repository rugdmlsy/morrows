use super::*;
use axum::extract::Query;

#[derive(Deserialize)]
struct RequestQuery {
    task_id: Option<Id>,
    agent_instance_id: Option<Id>,
    #[serde(default)]
    include_resolved: bool,
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_limit() -> i64 {
    20
}

#[derive(Deserialize)]
struct ResolveRequest {
    action: String,
    resolution: String,
    #[serde(default = "default_lease")]
    lease_seconds: i64,
}
fn default_lease() -> i64 {
    900
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/assignment-requests", get(list))
        .route("/assignment-requests/{id}/resolve", post(resolve))
}
async fn list(
    State(s): State<AppState>,
    Query(q): Query<RequestQuery>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store
            .assignment_requests_page(
                q.task_id,
                q.agent_instance_id,
                q.include_resolved,
                q.limit,
                q.offset
            )
            .await?
    )))
}
async fn resolve(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(body): Json<ResolveRequest>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store
            .resolve_assignment_request(
                id,
                None,
                &body.action,
                &body.resolution,
                body.lease_seconds
            )
            .await?
    )))
}
