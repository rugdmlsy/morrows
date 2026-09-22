use super::*;
use crate::collaboration::actor;
use axum::http::HeaderMap;
use morrows_core::*;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agent-profiles", get(profile_list).post(profile_register))
        .route("/agent-profiles/{id}", get(profile_get))
        .route("/accounts", get(account_list).post(account_register))
        .route("/accounts/{id}", get(account_get))
        .route("/machines", get(machine_list).post(machine_register))
        .route("/machines/{id}", get(machine_get))
        .route(
            "/agent-instances",
            post(instance_register).get(instance_list),
        )
        .route("/agent-instances/{id}", get(instance_get))
        .route("/agent-instances/{id}/heartbeat", post(heartbeat))
        .route(
            "/agent-instances/{id}/capacity",
            post(capacity_record).get(capacity_history),
        )
        .route(
            "/agent-instances/{id}/capacity/latest",
            get(capacity_latest),
        )
        .route("/agent-fleet", get(fleet))
}
async fn profile_register(
    State(s): State<AppState>,
    Json(input): Json<RegisterProfile>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.register_profile(input).await?)))
}
async fn profile_list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.list_profiles().await?)))
}
async fn profile_get(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.get_profile(id).await?)))
}
async fn account_register(
    State(s): State<AppState>,
    Json(input): Json<RegisterAccount>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.register_account(input).await?)))
}
async fn account_list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.list_accounts().await?)))
}
async fn account_get(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.get_account(id).await?)))
}
async fn machine_register(
    State(s): State<AppState>,
    Json(input): Json<RegisterMachine>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.register_machine(input).await?)))
}
async fn machine_list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.list_machines().await?)))
}
async fn machine_get(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.get_machine(id).await?)))
}
async fn instance_register(
    State(s): State<AppState>,
    Json(input): Json<RegisterAgentInstance>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.register_agent_instance(input).await?)))
}
async fn instance_list(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.list_agents().await?)))
}
async fn instance_get(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.get_agent(id).await?)))
}
async fn heartbeat(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
    Json(input): Json<AgentHeartbeat>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.agent_heartbeat(id, actor(&headers)?, input).await?
    )))
}
async fn capacity_record(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
    Json(input): Json<RecordCapacity>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.capacity_record(id, actor(&headers)?, input).await?
    )))
}
async fn capacity_latest(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.capacity_latest(id).await?)))
}
async fn capacity_history(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.capacity_history(id).await?)))
}
async fn fleet(State(s): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.agent_fleet().await?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use tower::ServiceExt;

    async fn request(
        app: &Router,
        method: &str,
        path: &str,
        actor: Option<Id>,
        body: Value,
    ) -> (StatusCode, Value) {
        let mut req = Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json");
        if let Some(id) = actor {
            req = req.header("x-agent-instance-id", id.to_string());
        }
        let res = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = res.status();
        let bytes = to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }
    #[tokio::test]
    async fn rest_normalized_registration_heartbeat_and_capacity() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let other = store.register_agent("other", &[]).await.unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });
        let mut ids = Vec::new();
        for (path, body) in [
            (
                "/agent-profiles",
                json!({"name":"Codex","provider":"openai","default_capabilities":["code"]}),
            ),
            ("/accounts", json!({"provider":"openai","label":"personal"})),
            ("/machines", json!({"name":"mac","hostname":"mac.local"})),
        ] {
            let (status, obj) = request(&app, "POST", path, None, body).await;
            assert_eq!(status, StatusCode::OK, "{obj}");
            let (status, got) = request(
                &app,
                "GET",
                &format!("{path}/{}", obj["id"].as_str().unwrap()),
                None,
                json!(null),
            )
            .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(got, obj);
            let (status, list) = request(&app, "GET", path, None, json!(null)).await;
            assert_eq!(status, StatusCode::OK);
            assert!(list.as_array().unwrap().contains(&obj));
            ids.push(obj["id"].clone());
        }
        let registration =
            json!({"name":"worker","profile_id":ids[0],"account_id":ids[1],"machine_id":ids[2]});
        let (status, worker) =
            request(&app, "POST", "/agent-instances", None, registration.clone()).await;
        assert_eq!(status, StatusCode::OK, "{worker}");
        assert_eq!(worker["capabilities"], json!(["code"]));
        assert_eq!(worker["profile_id"], ids[0]);
        assert_eq!(worker["account_id"], ids[1]);
        assert_eq!(worker["machine_id"], ids[2]);
        assert_eq!(
            request(&app, "POST", "/agent-instances", None, registration)
                .await
                .1["id"],
            worker["id"]
        );
        let id: Id = serde_json::from_value(worker["id"].clone()).unwrap();
        let base = format!("/agent-instances/{id}");
        assert_eq!(
            request(&app, "GET", &base, None, json!(null)).await.1["id"],
            worker["id"]
        );
        let cap = json!({"status":"ready","available_slots":1,"active_assignments":0,"active_runs":0,"max_concurrency":1});
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("{base}/capacity/latest"),
                None,
                json!(null)
            )
            .await
            .1,
            Value::Null
        );
        for (path, body) in [
            (
                format!("{base}/heartbeat"),
                json!({"status":"busy","capacity":cap}),
            ),
            (format!("{base}/capacity"), cap.clone()),
        ] {
            assert_eq!(
                request(&app, "POST", &path, None, body.clone()).await.0,
                StatusCode::BAD_REQUEST
            );
            assert_eq!(
                request(&app, "POST", &path, Some(other.id), body.clone())
                    .await
                    .0,
                StatusCode::CONFLICT
            );
            assert_eq!(
                request(&app, "POST", &path, Some(id), body).await.0,
                StatusCode::OK
            );
        }
        let (status, history) =
            request(&app, "GET", &format!("{base}/capacity"), None, json!(null)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(history.as_array().unwrap().len(), 2);
        let latest = request(
            &app,
            "GET",
            &format!("{base}/capacity/latest"),
            None,
            json!(null),
        )
        .await
        .1;
        assert_eq!(latest, history[0]);
        let mut invalid = cap;
        invalid["available_slots"] = json!(2);
        assert_eq!(
            request(&app, "POST", &format!("{base}/capacity"), Some(id), invalid)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        let fleet = request(&app, "GET", "/agent-fleet", None, json!(null))
            .await
            .1;
        let entry = fleet
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["instance"]["id"] == worker["id"])
            .unwrap();
        assert_eq!(entry["profile"]["id"], ids[0]);
        assert_eq!(entry["account"]["id"], ids[1]);
        assert_eq!(entry["machine"]["id"], ids[2]);
        assert_eq!(entry["instance"]["status"], "busy");
        assert_eq!(entry["latest_capacity"], latest);
        assert!(entry["machine"]["last_seen_at"].is_string());
    }
}
