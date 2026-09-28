use super::*;

pub fn routes() -> Router<AppState> {
    Router::new().route("/assignments/{id}/intake", get(get_intake))
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
