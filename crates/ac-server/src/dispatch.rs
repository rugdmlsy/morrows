use super::*;
use ac_core::*;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dispatch-policies", get(policy_list))
        .route("/tasks/{id}/dispatch-policy", post(policy_set))
        .route("/tasks/{id}/dispatch-policy/{role}", get(policy_get))
        .route("/tasks/{id}/dispatch-preview/{role}", get(preview))
        .route("/tasks/{id}/dispatch/{role}", post(dispatch_task))
        .route("/tasks/{id}/dispatch-decisions", get(decisions))
        .route("/dispatch/next/{role}", post(dispatch_next))
}

async fn policy_list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.list_dispatch_policies().await?)))
}
async fn policy_set(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<SetDispatchPolicy>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.set_dispatch_policy(id, input).await?)))
}
async fn policy_get(
    State(s): State<AppState>,
    Path((id, role)): Path<(Id, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.get_dispatch_policy(id, &role).await?)))
}
async fn preview(
    State(s): State<AppState>,
    Path((id, role)): Path<(Id, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.dispatch_preview(id, &role).await?)))
}
async fn dispatch_task(
    State(s): State<AppState>,
    Path((id, role)): Path<(Id, String)>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.dispatch_task(id, &role).await?)))
}
async fn decisions(State(s): State<AppState>, Path(id): Path<Id>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.task_dispatch_decisions(id).await?)))
}
async fn dispatch_next(
    State(s): State<AppState>,
    Path(role): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.dispatch_next(&role).await?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    async fn request(app: &Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn rest_dispatch_policy_preview_dispatch_and_history() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let profile = store
            .register_profile(
                serde_json::from_value(json!({
                    "name":"REST dispatch","provider":"openai",
                    "default_capabilities":["code"]
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let worker = store
            .register_agent_instance(
                serde_json::from_value(json!({
                    "name":"rest-dispatch-worker","profile_id":profile.id
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store.agent_heartbeat(worker.id,worker.id,serde_json::from_value(json!({
            "status":"online","capacity":{"status":"available","available_slots":1,"active_assignments":0,"active_runs":0,"max_concurrency":1}
        })).unwrap()).await.unwrap();
        let task = store
            .create_task(serde_json::from_value(json!({"title":"REST dispatch"})).unwrap())
            .await
            .unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });

        let policy_path = format!("/tasks/{}/dispatch-policy", task.id);
        let (status, policy) = request(
            &app,
            "POST",
            &policy_path,
            json!({
                "role":"executor","required_capabilities":["code"],"lease_seconds":300
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{policy}");
        assert_eq!(policy["enabled"], true);

        let get_path = format!("/tasks/{}/dispatch-policy/executor", task.id);
        assert_eq!(
            request(&app, "GET", &get_path, json!(null)).await.1["task_id"],
            json!(task.id)
        );

        let preview_path = format!("/tasks/{}/dispatch-preview/executor", task.id);
        let (status, preview) = request(&app, "GET", &preview_path, json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(preview["selected_agent_instance_id"], json!(worker.id));

        let dispatch_path = format!("/tasks/{}/dispatch/executor", task.id);
        let (status, outcome) = request(&app, "POST", &dispatch_path, json!(null)).await;
        assert_eq!(status, StatusCode::OK, "{outcome}");
        assert_eq!(outcome["assignment"]["agent_instance_id"], json!(worker.id));
        assert_eq!(outcome["decision"]["outcome"], "assigned");

        let decisions_path = format!("/tasks/{}/dispatch-decisions", task.id);
        let history = request(&app, "GET", &decisions_path, json!(null)).await.1;
        assert_eq!(history.as_array().unwrap().len(), 1);
        assert_eq!(history[0]["assignment_id"], outcome["assignment"]["id"]);
    }
}
