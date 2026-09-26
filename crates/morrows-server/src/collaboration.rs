use super::*;
use axum::http::HeaderMap;
use morrows_core::*;

pub(crate) fn actor(headers: &HeaderMap) -> Result<Id, ApiError> {
    headers
        .get("x-agent-instance-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or_else(|| {
            ApiError(DomainError::InvalidInput(
                "valid X-Agent-Instance-Id required".into(),
            ))
        })
}
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/tasks/{id}/collaboration", get(collaboration))
        .route("/handoffs/{id}", get(handoff_get))
        .route("/handoffs/{id}/accept", post(handoff_accept))
        .route(
            "/tasks/{id}/dependencies/{dependency}",
            axum::routing::delete(dependency_remove),
        )
        .route("/tasks/{id}/artifacts", post(artifact_create))
        .route("/tasks/{id}/decisions", post(decision_create))
        .route("/tasks/{id}/threads", post(thread_create))
        .route("/threads/{id}/messages", post(message_create))
        .route("/runs/{id}/handoffs", post(handoff_create))
        .route("/tasks/{id}/dependencies", post(dependency_add))
        .route(
            "/tasks/{id}/context-package",
            get(context_package_get).post(context_package_create),
        )
        .route(
            "/work-items/{id}/context-package",
            get(context_package_get).post(context_package_create),
        )
}
async fn collaboration(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.task_collaboration(id).await?)))
}
async fn handoff_get(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.get_handoff(id).await?)))
}
#[derive(Deserialize)]
struct AcceptHandoffBody {
    target_run_id: Id,
}
async fn handoff_accept(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
    Json(input): Json<AcceptHandoffBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store
            .accept_handoff(id, input.target_run_id, actor(&headers)?)
            .await?
    )))
}
#[derive(Deserialize)]

struct DependencyBody {
    depends_on_task_id: Id,
}
async fn dependency_add(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
    Json(input): Json<DependencyBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store
            .add_dependency(id, input.depends_on_task_id, actor(&headers)?)
            .await?
    )))
}
async fn dependency_remove(
    State(s): State<AppState>,
    Path((id, dependency)): Path<(Id, Id)>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    s.store
        .remove_dependency(id, dependency, actor(&headers)?)
        .await?;
    Ok(Json(json!({"removed":true})))
}
async fn artifact_create(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
    Json(input): Json<CreateArtifact>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.create_artifact(id, actor(&headers)?, input).await?
    )))
}
async fn decision_create(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
    Json(input): Json<CreateDecision>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.create_decision(id, actor(&headers)?, input).await?
    )))
}
async fn thread_create(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
    Json(input): Json<CreateThread>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.create_thread(id, actor(&headers)?, input).await?
    )))
}
async fn message_create(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
    Json(input): Json<CreateMessage>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.create_message(id, actor(&headers)?, input).await?
    )))
}
async fn handoff_create(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
    Json(input): Json<CreateHandoff>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.create_handoff(id, actor(&headers)?, input).await?
    )))
}

#[derive(Debug, Deserialize, Default)]
struct ContextPackageInput {
    #[serde(default)]
    pub objective: Option<String>,
    #[serde(default)]
    pub summary: Option<Value>,
    #[serde(default)]
    pub context_snapshot_id: Option<Id>,
    #[serde(default)]
    pub memory_refs: Vec<String>,
    #[serde(default)]
    pub decision_refs: Vec<Id>,
    #[serde(default)]
    pub artifact_refs: Vec<Id>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub verified_results: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub unresolved_questions: Vec<String>,
    #[serde(default)]
    pub next_action: Option<String>,
    #[serde(default)]
    pub source_run_id: Option<Id>,
    #[serde(default)]
    pub source_agent_id: Option<Id>,
    #[serde(default)]
    pub assemble: Option<bool>,
}

async fn context_package_get(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.get_latest_context_package(id).await?)))
}

async fn context_package_create(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ApiError> {
    let input: ContextPackageInput = if body.is_empty() {
        ContextPackageInput::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| DomainError::InvalidInput(e.to_string()))?
    };
    let agent_id = input.source_agent_id.or_else(|| actor(&headers).ok());
    if input.assemble == Some(true) || input.objective.is_none() {
        let pkg = s
            .store
            .assemble_context_package(id, input.source_run_id, agent_id)
            .await?;
        Ok(Json(json!(pkg)))
    } else {
        let objective = input.objective.unwrap();
        let pkg = s
            .store
            .create_context_package(CreateContextPackage {
                work_item_id: id,
                objective,
                summary: input.summary,
                context_snapshot_id: input.context_snapshot_id,
                memory_refs: input.memory_refs,
                decision_refs: input.decision_refs,
                artifact_refs: input.artifact_refs,
                changed_files: input.changed_files,
                verified_results: input.verified_results,
                blockers: input.blockers,
                unresolved_questions: input.unresolved_questions,
                next_action: input.next_action.unwrap_or_default(),
                source_run_id: input.source_run_id,
                source_agent_id: agent_id,
            })
            .await?;
        Ok(Json(json!(pkg)))
    }
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
        let response = app
            .clone()
            .oneshot(req.body(Body::from(body.to_string())).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn rest_collaboration_routes_validate_identity_and_persist_records() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("rest", &[]).await.unwrap();
        let task = store
            .create_task(serde_json::from_value(json!({"title":"REST"})).unwrap())
            .await
            .unwrap();
        store
            .create_context_revision(
                task.id,
                serde_json::from_value(json!({"goal":"continue"})).unwrap(),
            )
            .await
            .unwrap();
        let assignment = store
            .claim_task(task.id, agent.id, "executor", 300)
            .await
            .unwrap();
        let run = store
            .start_run(assignment.id, agent.id, None)
            .await
            .unwrap();
        let app = routes().with_state(AppState {
            store: store.clone(),
        });
        let artifact_path = format!("/tasks/{}/artifacts", task.id);
        let artifact = json!({"title":"patch","uri":"file:///patch"});
        assert_eq!(
            request(&app, "POST", &artifact_path, None, artifact.clone())
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        let (status, artifact) =
            request(&app, "POST", &artifact_path, Some(agent.id), artifact).await;
        assert_eq!(status, StatusCode::OK);
        let (status, decision) = request(
            &app,
            "POST",
            &format!("/tasks/{}/decisions", task.id),
            Some(agent.id),
            json!({"title":"choice","rationale":"reason"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, thread) = request(
            &app,
            "POST",
            &format!("/tasks/{}/threads", task.id),
            Some(agent.id),
            json!({"title":"notes"}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            request(
                &app,
                "POST",
                &format!("/threads/{}/messages", thread["id"].as_str().unwrap()),
                Some(agent.id),
                json!({"body":"test next"})
            )
            .await
            .0,
            StatusCode::OK
        );
        let (status,handoff)=request(&app,"POST",&format!("/runs/{}/handoffs",run.id),Some(agent.id),json!({"summary":"continue","completed":["patch"],"remaining":["test"],"artifact_ids":[artifact["id"]],"decision_ids":[decision["id"]]})).await;
        assert_eq!(status, StatusCode::OK);
        let (status, recovered) = request(
            &app,
            "GET",
            &format!("/handoffs/{}", handoff["id"].as_str().unwrap()),
            None,
            json!(null),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(recovered["context"]["goal"], "continue");
        assert_eq!(artifact["kind"], "other");
        assert_eq!(recovered["handoff"]["status"], "pending");
        let b = store.register_agent("rest-target", &[]).await.unwrap();
        let next = store
            .claim_task(task.id, b.id, "executor", 300)
            .await
            .unwrap();
        let target = store.start_run(next.id, b.id, None).await.unwrap();
        let accept_path = format!("/handoffs/{}/accept", handoff["id"].as_str().unwrap());
        let body = json!({"target_run_id":target.id});
        assert_eq!(
            request(&app, "POST", &accept_path, None, body.clone())
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            request(&app, "POST", &accept_path, Some(agent.id), body.clone())
                .await
                .0,
            StatusCode::CONFLICT
        );
        let (status, accepted) =
            request(&app, "POST", &accept_path, Some(b.id), body.clone()).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(accepted["status"], "accepted");
        assert_eq!(accepted["accepted_by_run_id"], json!(target.id));
        assert_eq!(
            request(&app, "POST", &accept_path, Some(b.id), body)
                .await
                .0,
            StatusCode::CONFLICT
        );

        let (status, all) = request(
            &app,
            "GET",
            &format!("/tasks/{}/collaboration", task.id),
            None,
            json!(null),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(all["messages"][0]["body"], "test next");
        assert_eq!(
            request(
                &app,
                "GET",
                &format!("/tasks/{}/collaboration", Uuid::new_v4()),
                None,
                json!(null)
            )
            .await
            .0,
            StatusCode::NOT_FOUND
        );
        let prerequisite = store
            .create_task(serde_json::from_value(json!({"title":"prerequisite"})).unwrap())
            .await
            .unwrap();
        let dep_path = format!("/tasks/{}/dependencies", task.id);
        let body = json!({"depends_on_task_id":prerequisite.id});
        assert_eq!(
            request(&app, "POST", &dep_path, Some(agent.id), body.clone())
                .await
                .0,
            StatusCode::OK
        );
        assert_eq!(
            request(&app, "POST", &dep_path, Some(agent.id), body)
                .await
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            request(
                &app,
                "DELETE",
                &format!("{dep_path}/{}", prerequisite.id),
                Some(agent.id),
                json!(null)
            )
            .await
            .0,
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn context_package_rest_routes_support_assembly_and_custom_creation() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("package-agent", &[]).await.unwrap();
        let task = store
            .create_task(
                serde_json::from_value(json!({
                    "title": "Migrate database",
                    "description": "Run phase 1 migrations cleanly"
                }))
                .unwrap(),
            )
            .await
            .unwrap();

        let app = routes().with_state(AppState {
            store: store.clone(),
        });

        // 1. Initial GET returns null
        let (status, initial_pkg) = request(
            &app,
            "GET",
            &format!("/tasks/{}/context-package", task.id),
            None,
            json!(null),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(initial_pkg.is_null());

        // Create an artifact and decision to test assembly
        let _ = store
            .create_decision(
                task.id,
                agent.id,
                CreateDecision {
                    title: "Use sqlite".into(),
                    rationale: "Embedded database".into(),
                },
            )
            .await
            .unwrap();

        // 2. POST without body assembles package
        let (status, assembled) = request(
            &app,
            "POST",
            &format!("/tasks/{}/context-package", task.id),
            Some(agent.id),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            assembled["objective"],
            "Migrate database: Run phase 1 migrations cleanly"
        );
        let decisions = assembled["decision_refs"].as_array().unwrap();
        assert_eq!(decisions.len(), 1);

        // 3. GET returns the assembled package
        let (status, latest) = request(
            &app,
            "GET",
            &format!("/tasks/{}/context-package", task.id),
            None,
            json!(null),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(latest["id"], assembled["id"]);

        // 4. POST with explicit custom package
        let (status, custom) = request(
            &app,
            "POST",
            &format!("/tasks/{}/context-package", task.id),
            Some(agent.id),
            json!({
                "objective": "Verify migration",
                "changed_files": ["migrations/0011.sql"],
                "verified_results": ["cargo test passed"],
                "next_action": "Deploy to staging"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(custom["objective"], "Verify migration");
        assert_eq!(custom["changed_files"][0], "migrations/0011.sql");
        assert_eq!(custom["verified_results"][0], "cargo test passed");

        // 5. GET via normalized /work-items/{id}/context-package alias returns latest custom package
        let (status, work_item_latest) = request(
            &app,
            "GET",
            &format!("/work-items/{}/context-package", task.id),
            None,
            json!(null),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(work_item_latest["id"], custom["id"]);
    }
}
