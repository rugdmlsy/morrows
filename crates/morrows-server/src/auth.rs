use super::*;
use axum::{
    body::Body,
    http::{HeaderValue, Method, Request, header::AUTHORIZATION},
    middleware::Next,
};
pub(crate) const MORROWS_OAUTH_VERIFIED_HEADER: &str = "x-morrows-oauth-verified";
pub(crate) const MORROWS_OAUTH_CLIENT_ID_HEADER: &str = "x-morrows-oauth-client-id";
pub(crate) const MORROWS_OAUTH_CLIENT_NAME_HEADER: &str = "x-morrows-oauth-client-name";
pub(crate) const AUTH_SOURCE_HEADER: &str = "x-morrows-auth-source";

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
    // Identity comes only from a locally verified control-plane token, never headers.
    let identity = match authorization
        .as_deref()
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        Some(token) => crate::oauth::verify(&state.store, token).await,
        None => None,
    };
    let morrows_oauth_verified = identity.is_some();
    let morrows_client_id = identity.as_ref().map(|i| i.client_id.clone());
    let morrows_client_name = identity.map(|i| i.client_name);
    request.headers_mut().remove(AUTH_SOURCE_HEADER);

    if morrows_oauth_verified && requires_agent_auth {
        let Some(client_id) = morrows_client_id else {
            return unauthorized("Morrows OAuth is missing validated client identity");
        };
        let agent = match state.store.resolve_morrows_oauth_agent(&client_id).await {
            Ok(value) => value,
            Err(_) => return unauthorized("Morrows OAuth client identity could not be resolved"),
        };
        let Ok(agent_value) = HeaderValue::from_str(&agent.id.to_string()) else {
            return unauthorized("invalid Morrows OAuth AgentInstance");
        };
        let Ok(client_value) = HeaderValue::from_str(&client_id) else {
            return unauthorized("invalid Morrows OAuth client identity");
        };
        request.headers_mut().remove(MORROWS_OAUTH_VERIFIED_HEADER);
        request
            .headers_mut()
            .insert(MORROWS_OAUTH_CLIENT_ID_HEADER, client_value);
        if let Some(client_name) = morrows_client_name {
            if let Ok(value) = HeaderValue::from_str(&client_name) {
                request
                    .headers_mut()
                    .insert(MORROWS_OAUTH_CLIENT_NAME_HEADER, value);
            } else {
                request
                    .headers_mut()
                    .remove(MORROWS_OAUTH_CLIENT_NAME_HEADER);
            }
        } else {
            request
                .headers_mut()
                .remove(MORROWS_OAUTH_CLIENT_NAME_HEADER);
        }
        request.headers_mut().insert(
            AUTH_SOURCE_HEADER,
            HeaderValue::from_static("morrows_oauth"),
        );
        request
            .headers_mut()
            .insert("x-agent-instance-id", agent_value);
        return next.run(request).await;
    }

    request.headers_mut().remove(MORROWS_OAUTH_VERIFIED_HEADER);
    request.headers_mut().remove(MORROWS_OAUTH_CLIENT_ID_HEADER);
    request
        .headers_mut()
        .remove(MORROWS_OAUTH_CLIENT_NAME_HEADER);

    // The public MCP resource is OAuth-only. The loopback /mcp compatibility
    // surface remains available to Run-bound internal Agent credentials and is
    // not exposed by the production edge.
    if request.uri().path() == "/morrows" || request.uri().path() == "/morrows/" {
        let mut response = unauthorized("Morrows OAuth bearer required");
        let resource = std::env::var("MORROWS_MCP_URL")
            .unwrap_or_else(|_| "https://mcp.xycdev.com/morrows".into());
        if let Ok(mut url) = reqwest::Url::parse(&resource) {
            let path = format!("/.well-known/oauth-protected-resource{}", url.path());
            url.set_path(&path);
            if let Ok(value) = HeaderValue::from_str(&format!("Bearer resource_metadata=\"{url}\""))
            {
                response.headers_mut().insert("www-authenticate", value);
            }
        }
        return response;
    }
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
            request.headers_mut().insert(
                AUTH_SOURCE_HEADER,
                HeaderValue::from_static("morrows_agent_credential"),
            );
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
    if path == "/mcp" || path.starts_with("/mcp/") || path == "/morrows" || path == "/morrows/" {
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
            .route("/mcp", get(subject))
            .route("/morrows", get(subject))
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
    async fn strict_mcp_maps_validated_morrows_client_to_stable_technical_identity() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let spoofed = store.register_agent("spoofed-agent", &[]).await.unwrap();
        let strict = app(store.clone(), true).await;

        let token_a = crate::oauth::tests::test_bearer(&store, "oauth-client-a").await;
        let token_b = crate::oauth::tests::test_bearer(&store, "oauth-client-b").await;
        let call = |token: &str| {
            Request::builder()
                .uri("/mcp")
                .header(MORROWS_OAUTH_VERIFIED_HEADER, "1")
                .header(MORROWS_OAUTH_CLIENT_ID_HEADER, "spoofed-client")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .header(MORROWS_OAUTH_CLIENT_NAME_HEADER, "ChatGPT")
                .header("x-agent-instance-id", spoofed.id.to_string())
                .body(Body::empty())
                .unwrap()
        };

        let first = strict.clone().oneshot(call(&token_a)).await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first_id = String::from_utf8(
            axum::body::to_bytes(first.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        let first_id = Id::parse_str(&first_id).unwrap();
        let first_agent = store.get_agent(first_id).await.unwrap();
        assert_eq!(
            first_agent.external_instance_ref.as_deref(),
            Some("oauth-client-a")
        );
        assert_ne!(first_agent.id, spoofed.id);

        let repeated = strict.clone().oneshot(call(&token_a)).await.unwrap();
        let repeated_id = String::from_utf8(
            axum::body::to_bytes(repeated.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_eq!(repeated_id, first_id.to_string());

        let other = strict.oneshot(call(&token_b)).await.unwrap();
        let other_id = String::from_utf8(
            axum::body::to_bytes(other.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert_ne!(other_id, first_id.to_string());
    }

    #[tokio::test]
    async fn strict_mcp_rejects_forwarded_identity_without_token() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let response = app(store, true)
            .await
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .header(MORROWS_OAUTH_VERIFIED_HEADER, "1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
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

    #[tokio::test]
    async fn public_mcp_rejects_agent_tokens_and_forged_headers_even_in_local_mode() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("internal-only", &[]).await.unwrap();
        let issued = store
            .issue_bridge_credential(agent.id, "internal", 600)
            .await
            .unwrap();
        for strict in [false, true] {
            for token in [issued.token.as_str(), "old-runtime-jwt", ""] {
                let response = app(store.clone(), strict)
                    .await
                    .oneshot(
                        Request::builder()
                            .uri("/morrows")
                            .header(AUTHORIZATION, format!("Bearer {token}"))
                            .header(MORROWS_OAUTH_VERIFIED_HEADER, "1")
                            .header(MORROWS_OAUTH_CLIENT_ID_HEADER, "forged")
                            .header("x-agent-instance-id", agent.id.to_string())
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
                assert!(
                    response.headers()["www-authenticate"]
                        .to_str()
                        .unwrap()
                        .contains("/.well-known/oauth-protected-resource/morrows")
                );
            }
        }

        let internal = app(store, true)
            .await
            .oneshot(
                Request::builder()
                    .uri("/mcp")
                    .header(AUTHORIZATION, format!("Bearer {}", issued.token))
                    .header("x-agent-instance-id", agent.id.to_string())
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(internal.status(), StatusCode::OK);
    }
}
