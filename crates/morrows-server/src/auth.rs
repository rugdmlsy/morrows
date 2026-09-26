use super::*;
use axum::{
    body::Body,
    http::{HeaderValue, Method, Request, header::AUTHORIZATION},
    middleware::Next,
};
#[derive(Clone)]
pub struct AgentAuthState {
    store: Store,
    require_agent_auth: bool,
}

impl AgentAuthState {
    pub fn new(store: Store, require_agent_auth: bool) -> Self {
        Self {
            store,
            require_agent_auth,
        }
    }
}

pub async fn authenticate_agent_requests(
    State(state): State<AgentAuthState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    let requires_agent_auth = is_agent_http_surface(request.method(), request.uri().path());
    let authorization = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let claimed_agent = request
        .headers()
        .get("x-agent-instance-id")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);

    if let Some(authorization) = authorization {
        let Some(token) = authorization.strip_prefix("Bearer ") else {
            return unauthorized("Authorization must use Bearer credentials");
        };
        if token.starts_with("mrw_agent_") {
            let credential = match state.store.verify_agent_token(token).await {
                Ok(value) => value,
                Err(_) => return unauthorized("invalid or expired Agent credential"),
            };
            if let Some(claimed) = claimed_agent {
                let Ok(claimed) = Id::parse_str(&claimed) else {
                    return unauthorized("invalid X-Agent-Instance-Id");
                };
                if claimed != credential.agent_instance_id {
                    return unauthorized(
                        "Agent credential subject does not match X-Agent-Instance-Id",
                    );
                }
            }
            let Ok(value) = HeaderValue::from_str(&credential.agent_instance_id.to_string()) else {
                return unauthorized("invalid Agent credential subject");
            };
            request.headers_mut().insert("x-agent-instance-id", value);
            request.extensions_mut().insert(credential);
        } else if state.require_agent_auth && (requires_agent_auth || claimed_agent.is_some()) {
            return unauthorized("Bearer Agent credential required");
        }
    } else if state.require_agent_auth && (requires_agent_auth || claimed_agent.is_some()) {
        return unauthorized("Bearer Agent credential required");
    }

    next.run(request).await
}

pub(crate) fn is_agent_http_surface(method: &Method, path: &str) -> bool {
    if path == "/mcp" || path.starts_with("/mcp/") {
        return true;
    }
    if path == "/api/agent-deliveries" && method == Method::GET {
        return true;
    }
    if path.starts_with("/api/agent-deliveries/") && method == Method::POST {
        return true;
    }
    if path == "/api/launch-attempts/external/accept" && method == Method::POST {
        return true;
    }
    if let Some(rest) = path.strip_prefix("/api/agent-instances/") {
        if rest.ends_with("/heartbeat") && method == Method::POST {
            return true;
        }
        if rest.ends_with("/capacity") && method == Method::POST {
            return true;
        }
        if rest.ends_with("/external-launches") && method == Method::GET {
            return true;
        }
    }
    false
}

fn unauthorized(message: &str) -> Response {
    (StatusCode::UNAUTHORIZED, Json(json!({"error": message}))).into_response()
}

#[derive(Debug, Deserialize)]
struct IssueBridgeCredentialBody {
    #[serde(default = "default_bridge_label")]
    label: String,
    #[serde(default = "default_bridge_ttl")]
    ttl_seconds: i64,
}

fn default_bridge_label() -> String {
    "provider bridge".into()
}

fn default_bridge_ttl() -> i64 {
    30 * 24 * 60 * 60
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/agents/{id}/credentials",
            get(list_credentials).post(issue_bridge_credential),
        )
        .route("/agent-credentials/{id}/revoke", post(revoke_credential))
}

async fn issue_bridge_credential(
    State(state): State<AppState>,
    Path(agent_id): Path<Id>,
    Json(input): Json<IssueBridgeCredentialBody>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state
            .store
            .issue_bridge_credential(agent_id, &input.label, input.ttl_seconds)
            .await?
    )))
}

async fn list_credentials(
    State(state): State<AppState>,
    Path(agent_id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(
        state.store.list_agent_credentials(agent_id).await?
    )))
}

async fn revoke_credential(
    State(state): State<AppState>,
    Path(id): Path<Id>,
) -> Result<Json<Value>, ApiError> {
    Ok(Json(json!(state.store.revoke_agent_credential(id).await?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, routing::get};
    use tower::ServiceExt;

    async fn subject(headers: axum::http::HeaderMap) -> String {
        headers
            .get("x-agent-instance-id")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("none")
            .to_owned()
    }

    async fn app(store: Store, require: bool) -> Router {
        Router::new()
            .route("/employee", get(subject))
            .route("/api/agent-deliveries", get(subject))
            .layer(axum::middleware::from_fn_with_state(
                AgentAuthState::new(store, require),
                authenticate_agent_requests,
            ))
    }

    #[tokio::test]
    async fn strict_auth_accepts_bearer_and_normalizes_subject() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("auth-agent", &[]).await.unwrap();
        let issued = store
            .issue_bridge_credential(agent.id, "test bridge", 600)
            .await
            .unwrap();
        let response = app(store, true)
            .await
            .oneshot(
                Request::builder()
                    .uri("/employee")
                    .header(AUTHORIZATION, format!("Bearer {}", issued.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), agent.id.to_string().as_bytes());
    }

    #[tokio::test]
    async fn strict_auth_rejects_legacy_header_mismatch_and_revoked_token() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = store.register_agent("auth-a", &[]).await.unwrap();
        let b = store.register_agent("auth-b", &[]).await.unwrap();
        let issued = store
            .issue_bridge_credential(a.id, "test bridge", 600)
            .await
            .unwrap();

        let strict = app(store.clone(), true).await;
        let legacy = strict
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/employee")
                    .header("x-agent-instance-id", a.id.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy.status(), StatusCode::UNAUTHORIZED);

        let mismatch = strict
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/employee")
                    .header(AUTHORIZATION, format!("Bearer {}", issued.token))
                    .header("x-agent-instance-id", b.id.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(mismatch.status(), StatusCode::UNAUTHORIZED);

        store
            .revoke_agent_credential(issued.credential.id)
            .await
            .unwrap();
        let revoked = strict
            .oneshot(
                Request::builder()
                    .uri("/employee")
                    .header(AUTHORIZATION, format!("Bearer {}", issued.token))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn strict_agent_surface_rejects_missing_bearer_even_without_identity_header() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let response = app(store, true)
            .await
            .oneshot(
                Request::builder()
                    .uri("/api/agent-deliveries")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn local_compat_mode_still_accepts_identity_header() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("legacy-agent", &[]).await.unwrap();
        let response = app(store, false)
            .await
            .oneshot(
                Request::builder()
                    .uri("/employee")
                    .header("x-agent-instance-id", agent.id.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
