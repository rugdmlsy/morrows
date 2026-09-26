use super::*;
use axum::{
    body::Body,
    http::{Method, Request, header::AUTHORIZATION},
    middleware::Next,
};

#[derive(Clone)]
pub struct OperatorAuthState {
    store: Store,
    require_operator_auth: bool,
    bootstrap_token: Option<String>,
}

impl OperatorAuthState {
    pub fn new(store: Store, require_operator_auth: bool, bootstrap_token: Option<String>) -> Self {
        Self {
            store,
            require_operator_auth,
            bootstrap_token,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ControlRole {
    Viewer,
    Operator,
    Admin,
}

impl ControlRole {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "viewer" => Some(Self::Viewer),
            "operator" => Some(Self::Operator),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

pub async fn authenticate_operator_requests(
    State(state): State<OperatorAuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_owned();

    if method == Method::OPTIONS
        || !path.starts_with("/api/")
        || path == "/api/health"
        || is_operator_login_route(&method, &path)
        || crate::auth::is_agent_http_surface(&method, &path)
    {
        return next.run(request).await;
    }

    let required = required_control_role(&method, &path);
    let authorization = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());

    let Some(authorization) = authorization else {
        if state.require_operator_auth {
            return unauthorized("Bearer operator credential required");
        }
        request.extensions_mut().insert(OperatorIdentity(
            json!({"authenticated":false,"role":"local","auth_required":false}),
        ));
        return next.run(request).await;
    };
    let Some(token) = authorization.strip_prefix("Bearer ") else {
        return unauthorized("Authorization must use Bearer credentials");
    };

    let mut identity =
        json!({"authenticated":true,"role":"admin","label":"bootstrap","expires_at":null});
    let role = if state.bootstrap_token.as_deref() == Some(token) {
        ControlRole::Admin
    } else if token.starts_with("mrw_operator_") {
        let credential = match state.store.verify_operator_token(token).await {
            Ok(value) => value,
            Err(_) => return unauthorized("invalid or expired operator credential"),
        };
        let Some(role) = ControlRole::parse(&credential.role) else {
            return forbidden("operator credential has an unknown role");
        };
        identity = json!({"authenticated":true,"role":credential.role,"label":credential.label,"expires_at":credential.expires_at});
        role
    } else if token.starts_with("mrw_agent_") {
        return forbidden("Agent credentials cannot access the control plane");
    } else {
        return unauthorized("invalid operator credential");
    };

    if role < required {
        return forbidden(match required {
            ControlRole::Viewer => "viewer role required",
            ControlRole::Operator => "operator role required",
            ControlRole::Admin => "admin role required",
        });
    }
    request.extensions_mut().insert(OperatorIdentity(identity));
    next.run(request).await
}

#[derive(Clone)]
struct OperatorIdentity(Value);

fn is_operator_login_route(method: &Method, path: &str) -> bool {
    // Neither endpoint approves anything. The random browser secret is required
    // to poll, and approval is available only through the host-local CLI.
    method == Method::POST
        && (path == "/api/operator-login" || path == "/api/operator-login/status")
}

fn required_control_role(method: &Method, path: &str) -> ControlRole {
    if is_admin_route(method, path) {
        return ControlRole::Admin;
    }
    if matches!(*method, Method::GET | Method::HEAD) {
        ControlRole::Viewer
    } else {
        ControlRole::Operator
    }
}

fn is_admin_route(method: &Method, path: &str) -> bool {
    if path == "/api/operator-credentials"
        || path.starts_with("/api/operator-credentials/")
        || path.starts_with("/api/agent-credentials/")
        || path.ends_with("/credentials")
    {
        return true;
    }
    if method != Method::POST {
        return false;
    }
    matches!(
        path,
        "/api/agents"
            | "/api/agent-profiles"
            | "/api/accounts"
            | "/api/machines"
            | "/api/agent-instances"
    )
}

fn unauthorized(message: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": message}))).into_response()
}

fn forbidden(message: &str) -> Response {
    (StatusCode::FORBIDDEN, Json(json!({"error": message}))).into_response()
}

#[derive(Debug, Deserialize)]
struct IssueOperatorCredentialBody {
    label: String,
    role: String,
    #[serde(default = "default_operator_ttl")]
    ttl_seconds: i64,
}

fn default_operator_ttl() -> i64 {
    30 * 24 * 60 * 60
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/operator-session", get(operator_session))
        .route("/operator-login", post(request_login))
        .route("/operator-login/status", post(login_status))
        .route(
            "/operator-credentials",
            get(list_credentials).post(issue_credential),
        )
        .route("/operator-credentials/{id}/revoke", post(revoke_credential))
}

async fn operator_session(
    axum::Extension(identity): axum::Extension<OperatorIdentity>,
) -> Json<Value> {
    Json(identity.0)
}

#[derive(Deserialize)]
struct RequestLoginBody {
    label: String,
}

async fn request_login(
    State(state): State<AppState>,
    Json(input): Json<RequestLoginBody>,
) -> Result<Response, ApiError> {
    let request = state.store.request_operator_login(&input.label).await?;
    Ok(([("Cache-Control", "no-store")], Json(json!(request))).into_response())
}

#[derive(Deserialize)]
struct LoginStatusBody {
    id: Id,
    token: String,
}

async fn login_status(
    State(state): State<AppState>,
    Json(input): Json<LoginStatusBody>,
) -> Result<Response, ApiError> {
    if input.token.len() > 256 {
        return Err(DomainError::InvalidInput("invalid login secret".into()).into());
    }
    let status = state
        .store
        .operator_login_status(input.id, &input.token)
        .await?;
    Ok(([("Cache-Control", "no-store")], Json(status)).into_response())
}

async fn issue_credential(
    State(state): State<AppState>,
    Json(input): Json<IssueOperatorCredentialBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .issue_operator_credential(&input.label, &input.role, input.ttl_seconds)
            .await?
    )))
}

async fn list_credentials(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(state.store.list_operator_credentials().await?)))
}

async fn revoke_credential(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state.store.revoke_operator_credential(id).await?
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::Request,
        routing::{get, post},
    };
    use tower::ServiceExt;

    async fn app(store: Store, require: bool, bootstrap: Option<String>) -> Router {
        Router::new()
            .route(
                "/api/tasks",
                get(|| async { "read" }).post(|| async { "write" }),
            )
            .route("/api/agent-profiles", post(|| async { "admin" }))
            .route("/api/agent-deliveries", get(|| async { "agent" }))
            .layer(axum::middleware::from_fn_with_state(
                OperatorAuthState::new(store, require, bootstrap),
                authenticate_operator_requests,
            ))
    }

    fn request(method: Method, path: &str, token: Option<&str>) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn browser_login_is_public_but_approval_and_control_remain_protected() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let app = Router::new()
            .nest("/api", routes())
            .with_state(AppState {
                store: store.clone(),
            })
            .layer(axum::middleware::from_fn_with_state(
                crate::auth::AgentAuthState::new(store.clone(), true),
                crate::auth::authenticate_agent_requests,
            ))
            .layer(axum::middleware::from_fn_with_state(
                OperatorAuthState::new(store.clone(), true, None),
                authenticate_operator_requests,
            ));
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/operator-login")
                    .header("Content-Type", "application/json")
                    .body(Body::from(r#"{"label":"Browser"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let login: morrows_core::OperatorLoginRequest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::GET,
                    "/api/operator-session",
                    Some(&login.token)
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, "/api/operator-login/approve", None))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        store
            .approve_operator_login(&login.code, "operator", 600)
            .await
            .unwrap();
        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                "/api/operator-session",
                Some(&login.token),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let identity: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(identity["authenticated"], true);
        assert_eq!(identity["role"], "operator");
        assert!(!String::from_utf8_lossy(&bytes).contains(&login.token));
        assert_eq!(
            app.oneshot(request(
                Method::GET,
                "/api/operator-credentials",
                Some(&login.token)
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn viewer_operator_admin_roles_are_enforced() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let viewer = store
            .issue_operator_credential("viewer", "viewer", 600)
            .await
            .unwrap();
        let operator = store
            .issue_operator_credential("operator", "operator", 600)
            .await
            .unwrap();
        let admin = store
            .issue_operator_credential("admin", "admin", 600)
            .await
            .unwrap();
        let app = app(store, true, None).await;

        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, "/api/tasks", Some(&viewer.token)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, "/api/tasks", Some(&viewer.token)))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(request(Method::POST, "/api/tasks", Some(&operator.token)))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    "/api/agent-profiles",
                    Some(&operator.token)
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    "/api/agent-profiles",
                    Some(&admin.token)
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn agent_surface_is_exempt_but_agent_token_cannot_enter_control_plane() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store
            .register_agent("operator-boundary-agent", &[])
            .await
            .unwrap();
        let agent_token = store
            .issue_bridge_credential(agent.id, "bridge", 600)
            .await
            .unwrap();
        let app = app(store, true, None).await;

        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::GET,
                    "/api/agent-deliveries",
                    Some(&agent_token.token),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.oneshot(request(Method::GET, "/api/tasks", Some(&agent_token.token)))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn bootstrap_token_is_admin_without_persistence() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let bootstrap = "mrw_operator_bootstrap-secret".to_owned();
        let app = app(store, true, Some(bootstrap.clone())).await;
        assert_eq!(
            app.oneshot(request(
                Method::POST,
                "/api/agent-profiles",
                Some(&bootstrap),
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn combined_middlewares_keep_agent_and_operator_credentials_separate() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store
            .register_agent("combined-auth-agent", &[])
            .await
            .unwrap();
        let agent_credential = store
            .issue_bridge_credential(agent.id, "combined bridge", 600)
            .await
            .unwrap();
        let operator_credential = store
            .issue_operator_credential("combined operator", "operator", 600)
            .await
            .unwrap();

        let app = Router::new()
            .route("/api/tasks", get(|| async { "control" }))
            .route("/api/agent-deliveries", get(|| async { "agent" }))
            .layer(axum::middleware::from_fn_with_state(
                crate::auth::AgentAuthState::new(store.clone(), true),
                crate::auth::authenticate_agent_requests,
            ))
            .layer(axum::middleware::from_fn_with_state(
                OperatorAuthState::new(store, true, None),
                authenticate_operator_requests,
            ));

        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, "/api/tasks", None))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request(Method::GET, "/api/agent-deliveries", None,))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::GET,
                    "/api/agent-deliveries",
                    Some(&agent_credential.token),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::GET,
                    "/api/tasks",
                    Some(&agent_credential.token),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            app.clone()
                .oneshot(request(
                    Method::GET,
                    "/api/tasks",
                    Some(&operator_credential.token),
                ))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.oneshot(request(
                Method::GET,
                "/api/agent-deliveries",
                Some(&operator_credential.token),
            ))
            .await
            .unwrap()
            .status(),
            StatusCode::UNAUTHORIZED
        );
    }
}
