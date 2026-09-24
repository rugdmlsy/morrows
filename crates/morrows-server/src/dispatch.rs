use super::*;
use morrows_core::*;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/dispatch-policies", get(policy_list))
        .route(
            "/dispatch-scheduler/{role}",
            get(scheduler_get).post(scheduler_set),
        )
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

async fn scheduler_get(
    State(s): State<AppState>,
    Path(role): Path<String>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.get_dispatch_scheduler_settings(&role).await?
    )))
}

async fn scheduler_set(
    State(s): State<AppState>,
    Path(role): Path<String>,
    Json(input): Json<SetDispatchSchedulerSettings>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store
            .set_dispatch_scheduler_settings(&role, input)
            .await?
    )))
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

async fn auto_launch_assignment(store: &Store, assignment: &Assignment) {
    if assignment.role != "executor" {
        return;
    }
    let profiles = match store.list_launch_profiles().await {
        Ok(profiles) => profiles
            .into_iter()
            .filter(|profile| {
                profile.enabled && profile.agent_instance_id == assignment.agent_instance_id
            })
            .collect::<Vec<_>>(),
        Err(err) => {
            tracing::error!(
                assignment_id = %assignment.id,
                %err,
                "scheduler could not list launch profiles"
            );
            return;
        }
    };
    let defaults = profiles
        .iter()
        .filter(|profile| {
            profile
                .metadata
                .get("default")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    let selected = if defaults.len() == 1 {
        Some(defaults[0].id)
    } else if defaults.is_empty() && profiles.len() == 1 {
        Some(profiles[0].id)
    } else {
        None
    };
    let Some(launch_profile_id) = selected else {
        tracing::warn!(
            assignment_id = %assignment.id,
            agent_instance_id = %assignment.agent_instance_id,
            enabled_profiles = profiles.len(),
            default_profiles = defaults.len(),
            "scheduler assigned work but could not determine a unique launch profile"
        );
        return;
    };
    if let Err(err) = store
        .enqueue_launch(EnqueueLaunch {
            assignment_id: assignment.id,
            launch_profile_id,
            cwd: None,
            resume_from_attempt_id: None,
        })
        .await
    {
        tracing::error!(
            assignment_id = %assignment.id,
            %launch_profile_id,
            %err,
            "scheduler failed to enqueue launch"
        );
    }
}

pub async fn scheduler_loop(store: Store) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    let mut last_run = std::collections::HashMap::<String, std::time::Instant>::new();
    loop {
        ticker.tick().await;
        let settings = match store.list_dispatch_scheduler_settings().await {
            Ok(settings) => settings,
            Err(err) => {
                tracing::error!(%err, "dispatch scheduler settings scan failed");
                continue;
            }
        };
        for setting in settings.into_iter().filter(|setting| setting.enabled) {
            let due = last_run
                .get(&setting.role)
                .is_none_or(|last| last.elapsed().as_secs() >= setting.interval_seconds as u64);
            if !due {
                continue;
            }
            last_run.insert(setting.role.clone(), std::time::Instant::now());

            for _ in 0..8 {
                let result = match store.dispatch_next_scheduled(&setting.role).await {
                    Ok(result) => result,
                    Err(err) => {
                        tracing::error!(
                            role = %setting.role,
                            %err,
                            "automatic dispatch failed"
                        );
                        break;
                    }
                };
                let Some(outcome) = result.dispatched else {
                    break;
                };
                if setting.auto_launch
                    && let Some(assignment) = outcome.assignment.as_ref()
                {
                    auto_launch_assignment(&store, assignment).await;
                }
            }
        }
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

    #[tokio::test]
    async fn scheduler_assigns_and_auto_launches_only_unique_profile() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let profile = store
            .register_profile(
                serde_json::from_value(json!({
                    "name":"scheduler profile",
                    "provider":"openai",
                    "default_capabilities":["code"]
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let worker = store
            .register_agent_instance(
                serde_json::from_value(json!({
                    "name":"scheduler-worker",
                    "profile_id":profile.id
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .agent_heartbeat(
                worker.id,
                worker.id,
                serde_json::from_value(json!({
                    "status":"online",
                    "capacity":{
                        "status":"available",
                        "available_slots":1,
                        "active_assignments":0,
                        "active_runs":0,
                        "max_concurrency":1
                    }
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .register_launch_profile(
                serde_json::from_value(json!({
                    "name":"unique local launcher",
                    "adapter":"codex_cli",
                    "agent_instance_id":worker.id,
                    "program":"/bin/echo",
                    "default_cwd":"/tmp",
                    "enabled":true,
                    "metadata":{"default":true}
                }))
                .unwrap(),
            )
            .await
            .unwrap();

        let task = store
            .create_task(serde_json::from_value(json!({"title":"auto dispatch"})).unwrap())
            .await
            .unwrap();
        store
            .set_dispatch_policy(
                task.id,
                serde_json::from_value(json!({
                    "role":"executor",
                    "required_capabilities":["code"],
                    "lease_seconds":300,
                    "enabled":true
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .set_dispatch_scheduler_settings(
                "executor",
                SetDispatchSchedulerSettings {
                    enabled: true,
                    interval_seconds: 1,
                    auto_launch: true,
                },
            )
            .await
            .unwrap();

        let handle = tokio::spawn(scheduler_loop(store.clone()));
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        handle.abort();

        let assignments = store.task_assignments(task.id).await.unwrap();
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].agent_instance_id, worker.id);
        let attempts = store.task_launch_attempts(task.id).await.unwrap();
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].assignment_id, assignments[0].id);
        assert_eq!(attempts[0].status, "queued");
    }
}
