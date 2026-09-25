use super::*;
use crate::collaboration::actor;
use axum::http::HeaderMap;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use morrows_core::*;
use std::path::{Path as FsPath, PathBuf};

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
struct CreateManagedCodexAgent {
    auth_json: String,
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
    let email = parse_codex_auth_email(&input.auth_json)?;

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
        .find(|item| item.adapter == "codex_cli" && item.enabled);
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
    if !FsPath::new(&default_cwd).is_absolute() || !FsPath::new(&default_cwd).is_dir() {
        return Err(DomainError::InvalidInput(format!(
            "default_cwd must be an existing absolute directory: {default_cwd}"
        ))
        .into());
    }

    let program = resolve_codex_program(template.map(|item| item.program.as_str()))?;
    let accounts_root = managed_codex_accounts_root()?;
    let codex_home = accounts_root.join(Uuid::new_v4().to_string());
    prepare_managed_codex_home(&codex_home, &input.auth_json)?;

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
            codex_home.to_string_lossy().as_ref(),
            &default_cwd,
            model,
        )
        .await;
    let (account, instance, launch_profile) = match provisioned {
        Ok(value) => value,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&codex_home);
            return Err(err.into());
        }
    };

    Ok(Json(json!({
        "provider": "codex",
        "account": account,
        "instance": instance,
        "launch_profile": launch_profile,
        "codex_home": codex_home,
        "credential_backend": "codex_home_file",
        "auth_imported": true,
    })))
}

fn parse_codex_auth_email(auth_json: &str) -> Result<String, ApiError> {
    if auth_json.len() > 512 * 1024 {
        return Err(DomainError::InvalidInput("Codex auth.json is too large".into()).into());
    }
    let auth: Value = serde_json::from_str(auth_json)
        .map_err(|_| DomainError::InvalidInput("selected file is not valid JSON".into()))?;
    let id_token = auth
        .get("tokens")
        .and_then(|tokens| tokens.get("id_token"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            DomainError::InvalidInput(
                "selected file is not a Codex auth.json: tokens.id_token is missing".into(),
            )
        })?;
    let payload = id_token
        .split('.')
        .nth(1)
        .ok_or_else(|| DomainError::InvalidInput("Codex id_token is not a valid JWT".into()))?;
    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| DomainError::InvalidInput("Codex id_token payload is invalid".into()))?;
    let claims: Value = serde_json::from_slice(&decoded)
        .map_err(|_| DomainError::InvalidInput("Codex id_token claims are invalid".into()))?;
    if claims.get("email_verified").and_then(Value::as_bool) == Some(false) {
        return Err(DomainError::InvalidInput("Codex account email is not verified".into()).into());
    }
    let email = claims
        .get("email")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.contains('@')
                && !value.chars().any(char::is_whitespace)
                && value.chars().count() <= 320
        })
        .ok_or_else(|| {
            DomainError::InvalidInput("Codex id_token does not contain a valid email".into())
        })?;
    Ok(email.to_ascii_lowercase())
}

fn prepare_managed_codex_home(codex_home: &FsPath, auth_json: &str) -> Result<(), ApiError> {
    if let Err(err) = std::fs::create_dir_all(codex_home) {
        return Err(DomainError::Storage(format!(
            "failed creating managed CODEX_HOME {}: {err}",
            codex_home.display()
        ))
        .into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) =
            std::fs::set_permissions(codex_home, std::fs::Permissions::from_mode(0o700))
        {
            let _ = std::fs::remove_dir_all(codex_home);
            return Err(DomainError::Storage(format!(
                "failed securing managed CODEX_HOME {}: {err}",
                codex_home.display()
            ))
            .into());
        }
    }

    let config_path = codex_home.join("config.toml");
    if let Err(err) = std::fs::write(&config_path, "cli_auth_credentials_store = \"file\"\n") {
        let _ = std::fs::remove_dir_all(codex_home);
        return Err(DomainError::Storage(format!(
            "failed writing managed Codex config {}: {err}",
            config_path.display()
        ))
        .into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) =
            std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600))
        {
            let _ = std::fs::remove_dir_all(codex_home);
            return Err(DomainError::Storage(format!(
                "failed securing managed Codex config {}: {err}",
                config_path.display()
            ))
            .into());
        }
    }

    let auth_path = codex_home.join("auth.json");
    if let Err(err) = std::fs::write(&auth_path, auth_json) {
        let _ = std::fs::remove_dir_all(codex_home);
        return Err(DomainError::Storage(format!(
            "failed writing managed Codex auth file {}: {err}",
            auth_path.display()
        ))
        .into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) =
            std::fs::set_permissions(&auth_path, std::fs::Permissions::from_mode(0o600))
        {
            let _ = std::fs::remove_dir_all(codex_home);
            return Err(DomainError::Storage(format!(
                "failed securing managed Codex auth file {}: {err}",
                auth_path.display()
            ))
            .into());
        }
    }
    Ok(())
}

fn managed_codex_accounts_root() -> Result<PathBuf, DomainError> {
    let root = std::env::var("MORROWS_CODEX_ACCOUNTS_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("data")
                .join("codex-accounts")
        });
    if !root.is_absolute() {
        return Err(DomainError::InvalidInput(
            "MORROWS_CODEX_ACCOUNTS_DIR must be absolute".into(),
        ));
    }
    Ok(root)
}

fn resolve_codex_program(template_program: Option<&str>) -> Result<String, DomainError> {
    if let Some(program) = std::env::var("MORROWS_CODEX_PROGRAM")
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
    {
        if FsPath::new(&program).is_absolute() && FsPath::new(&program).is_file() {
            return Ok(program);
        }
        return Err(DomainError::InvalidInput(format!(
            "MORROWS_CODEX_PROGRAM is not an existing absolute file: {program}"
        )));
    }

    if let Some(home) = std::env::var_os("HOME") {
        let candidate = PathBuf::from(home).join(".local/bin/codex");
        if candidate.is_file() {
            return Ok(candidate.to_string_lossy().into_owned());
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("codex");
            if candidate.is_file() {
                return Ok(candidate.to_string_lossy().into_owned());
            }
        }
    }
    if let Some(program) = template_program
        && FsPath::new(program)
            .file_name()
            .is_some_and(|name| name == "codex")
        && FsPath::new(program).is_file()
    {
        return Ok(program.to_owned());
    }
    Err(DomainError::InvalidState(
        "could not resolve the base Codex CLI; set MORROWS_CODEX_PROGRAM".into(),
    ))
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
    async fn managed_codex_rest_imports_auth_email_and_file() {
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
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake_program, std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        store
            .register_launch_profile(
                serde_json::from_value(json!({
                    "name":"seed-codex-launch",
                    "adapter":"codex_cli",
                    "agent_instance_id":seed_agent.id,
                    "program":fake_program,
                    "codex_home":null,
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
        let auth = fake_codex_auth("Imported@Example.com", true);
        let (status, body) = request(
            &app,
            "POST",
            "/agent-fleet/codex",
            None,
            json!({
                "auth_json":auth,
                "display_name":"imported-codex"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["account"]["email"], "imported@example.com");
        assert_eq!(body["instance"]["display_name"], "imported-codex");
        assert_eq!(body["auth_imported"], true);
        let codex_home = PathBuf::from(body["codex_home"].as_str().unwrap());
        assert_eq!(
            std::fs::read_to_string(codex_home.join("auth.json")).unwrap(),
            auth
        );
        let _ = std::fs::remove_dir_all(codex_home);
        let _ = std::fs::remove_dir_all(temp);
    }

    fn fake_codex_auth(email: &str, verified: bool) -> String {
        let payload = URL_SAFE_NO_PAD.encode(
            json!({"email":email,"email_verified":verified})
                .to_string()
                .as_bytes(),
        );
        json!({
            "auth_mode":"chatgpt",
            "tokens":{"id_token":format!("header.{payload}.signature")}
        })
        .to_string()
    }

    #[test]
    fn codex_auth_parser_extracts_verified_email() {
        assert_eq!(
            parse_codex_auth_email(&fake_codex_auth("User@Example.com", true)).unwrap(),
            "user@example.com"
        );
        assert!(parse_codex_auth_email(&fake_codex_auth("user@example.com", false)).is_err());
        assert!(parse_codex_auth_email("{}").is_err());
    }

    #[test]
    fn managed_codex_home_imports_auth_and_uses_private_permissions() {
        let root = std::env::temp_dir().join(format!("morrows-codex-home-{}", Uuid::new_v4()));
        let auth = fake_codex_auth("user@example.com", true);
        prepare_managed_codex_home(&root, &auth).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("config.toml")).unwrap(),
            "cli_auth_credentials_store = \"file\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("auth.json")).unwrap(),
            auth
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
                0o700
            );
            for path in [root.join("config.toml"), root.join("auth.json")] {
                assert_eq!(
                    std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
            }
        }
        let _ = std::fs::remove_dir_all(root);
    }
}
