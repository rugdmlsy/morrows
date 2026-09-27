use super::*;

#[derive(Deserialize)]
struct ResolveInterview {
    action: String,
    #[serde(default)]
    response: String,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/assignments/{id}/intake", get(get_intake))
        .route(
            "/assignments/{id}/intake/interview",
            post(resolve_interview),
        )
}

async fn get_intake(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    let assignment = s.store.get_assignment(id).await?;
    let intake = s.store.get_assignment_intake(id).await?;
    let blockers = s.store.intake_blockers(&assignment, &intake).await?;
    Ok(Json(json!({
        "assignment": assignment,
        "intake": intake,
        "blockers": blockers,
        "execution_ready": blockers.is_empty(),
    })))
}

async fn resolve_interview(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(body): Json<ResolveInterview>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store
            .resolve_intake_interview(
                id,
                body.action.trim(),
                body.response.trim(),
                "control-plane",
            )
            .await?
    )))
}
