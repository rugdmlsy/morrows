use super::*;
use crate::collaboration::actor;
use axum::http::HeaderMap;
use morrows_core::*;
use std::path::{Path as FsPath, PathBuf};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/agent-profiles", get(profile_list).post(profile_register))
        .route("/agent-profiles/{id}", get(profile_get))
        .route("/accounts", get(account_list).post(account_register))
        .route("/accounts/{id}", get(account_get))
        .route(
            "/accounts/{id}/credential-ref",
            post(account_credential_ref_set),
        )
        .route("/machines", get(machine_list).post(machine_register))
        .route("/machines/{id}", get(machine_get))
        .route(
            "/agent-instances",
            post(instance_register).get(instance_list),
        )
        .route("/agent-instances/{id}", get(instance_get))
        .route("/agent-instances/{id}/rename", post(instance_rename))
        .route("/agent-instances/{id}/archive", post(instance_archive))
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
        .route("/agent-fleet/codex", post(managed_codex_create))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateManagedCodexAgent {
    email: String,
    credential_ref: String,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    machine_id: Option<Id>,
    #[serde(default)]
    default_cwd: Option<String>,
    #[serde(default)]
    model: Option<String>,
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
async fn account_credential_ref_set(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<SetAccountCredentialRef>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.set_account_credential_ref(id, input).await?
    )))
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
async fn instance_rename(
    State(s): State<AppState>,
    Path(id): Path<Id>,
    Json(input): Json<RenameAgentInstance>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        s.store.rename_agent(id, &input.display_name).await?
    )))
}
async fn instance_archive(
    State(s): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(s.store.archive_agent(id).await?)))
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

async fn managed_codex_create(
    State(s): State<AppState>,
    Json(input): Json<CreateManagedCodexAgent>,
) -> Result<Json<Value>, ApiError> {
    let email = input.email.trim().to_ascii_lowercase();
    let credential_ref = input.credential_ref.trim();
    if email.is_empty() || credential_ref.is_empty() {
        return Err(
            DomainError::InvalidInput("email and credential_ref are required".into()).into(),
        );
    }

    let profile = if let Some(profile) = s
        .store
        .list_profiles()
        .await?
        .into_iter()
        .find(|profile| profile.provider == "openai" && profile.name == "Codex CLI")
    {
        profile
    } else {
        s.store
            .register_profile(RegisterProfile {
                name: "Codex CLI".into(),
                provider: "openai".into(),
                kind: "coding_agent".into(),
                default_capabilities: vec![
                    "rust".into(),
                    "typescript".into(),
                    "code".into(),
                    "review".into(),
                    "mcp".into(),
                ],
                metadata: json!({"managed_by":"morrows"}),
            })
            .await?
    };

    let launch_profiles = s.store.list_launch_profiles().await?;
    let template = launch_profiles
        .iter()
        .filter(|item| item.adapter == "codex_cli" && item.enabled)
        .find(|item| {
            FsPath::new(&item.program)
                .file_name()
                .is_some_and(|name| name == "codex")
        })
        .or_else(|| {
            launch_profiles
                .iter()
                .find(|item| item.adapter == "codex_cli" && item.enabled)
        });
    let fleet = s.store.agent_fleet().await?;
    let machine_id = input.machine_id.or_else(|| {
        fleet
            .iter()
            .find(|entry| entry.profile.id == profile.id)
            .and_then(|entry| entry.machine.as_ref().map(|machine| machine.id))
    });

    let default_cwd = input
        .default_cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| template.and_then(|item| item.default_cwd.clone()))
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .to_string_lossy()
                .into_owned()
        });
    if !FsPath::new(&default_cwd).is_absolute() {
        return Err(DomainError::InvalidInput(format!(
            "default_cwd must be an absolute path on the target machine: {default_cwd}"
        ))
        .into());
    }

    let program = resolve_codex_program(template.map(|item| item.program.as_str()))?;

    let display_name = input
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let model = input
        .model
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let provisioned = s
        .store
        .provision_managed_codex_agent(
            profile.id,
            machine_id,
            &email,
            display_name,
            &program,
            credential_ref,
            &default_cwd,
            model,
        )
        .await;
    let (account, instance, launch_profile) = provisioned?;

    Ok(Json(json!({
        "provider": "codex",
        "account": account,
        "instance": instance,
        "launch_profile": launch_profile,
        "credential_kind": "codex_home",
        "credential_ref": credential_ref,
        "credential_secret_stored": false,
    })))
}

fn resolve_codex_program(template_program: Option<&str>) -> Result<String, DomainError> {
    let program = std::env::var("MORROWS_CODEX_PROGRAM")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            template_program
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .ok_or_else(|| {
            DomainError::InvalidState(
                "no Codex CLI program reference is configured for the target machine".into(),
            )
        })?;
    if !FsPath::new(&program).is_absolute() {
        return Err(DomainError::InvalidInput(format!(
            "Codex CLI program must be an absolute path on the target machine: {program}"
        )));
    }
    Ok(program)
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

    #[tokio::test]
    async fn managed_codex_rest_links_external_credential_reference_without_secret_copy() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let profile = store
            .register_profile(
                serde_json::from_value(json!({
                    "name":"Codex CLI",
                    "provider":"openai",
                    "kind":"coding_agent",
                    "default_capabilities":["code","mcp"]
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let machine = store
            .register_machine(
                serde_json::from_value(json!({
                    "name":"mac",
                    "hostname":"mac.local",
                    "os":"macos",
                    "arch":"arm64"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let seed_account = store
            .register_account(
                serde_json::from_value(json!({
                    "provider":"openai",
                    "label":"seed",
                    "email":"seed@example.com"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let seed_agent = store
            .register_agent_instance(
                serde_json::from_value(json!({
                    "name":"seed-codex",
                    "profile_id":profile.id,
                    "account_id":seed_account.id,
                    "machine_id":machine.id
                }))
                .unwrap(),
            )
            .await
            .unwrap();

        let temp = std::env::temp_dir().join(format!("morrows-codex-api-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&temp).unwrap();
        let fake_program = temp.join("codex");
        std::fs::write(&fake_program, "#!/bin/sh\nexit 0\n").unwrap();
        store
            .register_launch_profile(
                serde_json::from_value(json!({
                    "name":"seed-codex-launch",
                    "adapter":"codex_cli",
                    "agent_instance_id":seed_agent.id,
                    "program":fake_program,
                    "default_cwd":temp,
                    "model":null,
                    "enabled":true
                }))
                .unwrap(),
            )
            .await
            .unwrap();

        let app = routes().with_state(AppState {
            store: store.clone(),
        });
        let (status, body) = request(
            &app,
            "POST",
            "/agent-fleet/codex",
            None,
            json!({
                "email":"Imported@Example.com",
                "credential_ref":"~/.codex",
                "display_name":"imported-codex"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["account"]["email"], "imported@example.com");
        assert_eq!(body["account"]["credential_kind"], "codex_home");
        assert_eq!(body["account"]["credential_ref"], "~/.codex");
        assert_eq!(body["instance"]["display_name"], "imported-codex");
        assert_eq!(body["credential_kind"], "codex_home");
        assert_eq!(body["credential_ref"], "~/.codex");
        assert_eq!(body["credential_secret_stored"], false);
        assert!(body.get("auth_imported").is_none());
        assert!(body.get("codex_home").is_none());

        let legacy = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/agent-fleet/codex")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "email":"other@example.com",
                            "credential_ref":"~/.codex-other",
                            "auth_json":"must-not-be-accepted"
                        })
                        .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let _ = std::fs::remove_dir_all(temp);
    }
}
