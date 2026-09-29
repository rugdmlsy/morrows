//! Public OAuth is control-plane state. Only hashes of bearer secrets are persisted.
use super::*;
use axum::{
    extract::{Form, Query},
    http::{HeaderMap, HeaderValue, header::CONTENT_TYPE},
    response::Html,
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const RESOURCE: &str = "https://mcp.xycdev.com/morrows";
const ACCESS_TTL: i64 = 3600;
const FAMILY_TTL: i64 = 90 * 86400;
const CONSENT_JS: &str = include_str!("oauth_consent.js");
#[derive(Clone)]
pub struct OAuthState {
    pub store: Store,
    pub resource: String,
    pub issuer: String,
}
impl OAuthState {
    pub fn from_env(store: Store) -> anyhow::Result<Self> {
        let resource = env::var("MORROWS_MCP_URL")
            .unwrap_or(RESOURCE.into())
            .trim_end_matches('/')
            .to_owned();
        let issuer = env::var("MORROWS_OAUTH_ISSUER").unwrap_or(format!("{resource}/auth"));
        for url in [&resource, &issuer] {
            let parsed = reqwest::Url::parse(url)?;
            anyhow::ensure!(
                parsed.scheme() == "https"
                    && parsed.host_str().is_some()
                    && parsed.query().is_none()
                    && parsed.fragment().is_none(),
                "public OAuth URLs must be absolute HTTPS URLs"
            );
        }
        Ok(Self {
            store,
            resource,
            issuer: issuer.trim_end_matches('/').into(),
        })
    }
}
#[derive(Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    clients: BTreeMap<String, Client>,
    #[serde(default)]
    codes: BTreeMap<String, Code>,
    #[serde(default)]
    grants: BTreeMap<String, Grant>,
    #[serde(default)]
    tokens: BTreeMap<String, Token>,
    #[serde(default)]
    prompts: BTreeMap<String, Prompt>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Client {
    name: String,
    redirects: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Prompt {
    expires: i64,
    binding: String,
}
#[derive(Serialize, Deserialize)]
struct Code {
    scope: String,
    client: String,
    redirect: String,
    challenge: String,
    expires: i64,
}
#[derive(Serialize, Deserialize)]
struct Grant {
    scope: String,
    client: String,
    expires: i64,
    revoked: bool,
}
#[derive(Serialize, Deserialize)]
struct Token {
    family: String,
    expires: i64,
    refresh: bool,
    used: bool,
}
#[derive(Clone, Debug)]
pub struct Identity {
    pub client_id: String,
    pub client_name: String,
    pub control: bool,
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn secret() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn hash(s: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(s.as_bytes()))
}
fn legacy_audience_matches(claims: &Value, resource: &str) -> bool {
    match claims.get("aud") {
        Some(Value::String(value)) => value == resource,
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(resource)),
        _ => false,
    }
}
fn verify_legacy_hs256(
    bearer: &str,
    secret: &str,
    issuer: &str,
    resource: &str,
) -> Option<Identity> {
    let mut parts = bearer.split('.');
    let header_part = parts.next()?;
    let claims_part = parts.next()?;
    let signature_part = parts.next()?;
    if parts.next().is_some() || secret.len() < 32 {
        return None;
    }

    let header: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header_part).ok()?).ok()?;
    if header.get("alg").and_then(Value::as_str) != Some("HS256") {
        return None;
    }
    let signature = URL_SAFE_NO_PAD.decode(signature_part).ok()?;
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(format!("{header_part}.{claims_part}").as_bytes());
    mac.verify_slice(&signature).ok()?;

    let claims: Value = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims_part).ok()?).ok()?;
    if claims.get("iss").and_then(Value::as_str) != Some(issuer)
        || !legacy_audience_matches(&claims, resource)
    {
        return None;
    }
    let issued = claims.get("iat").and_then(Value::as_i64)?;
    let current = now();
    if issued > current + 300 {
        return None;
    }
    if claims
        .get("exp")
        .and_then(Value::as_i64)
        .is_some_and(|expires| expires <= current)
    {
        return None;
    }
    let client_id = claims.get("client_id")?.as_str()?.trim();
    if client_id.is_empty()
        || client_id.chars().count() > 256
        || client_id.chars().any(char::is_control)
    {
        return None;
    }
    Some(Identity {
        client_id: client_id.to_owned(),
        client_name: "Legacy Morrows OAuth".into(),
        // Before the cutover, verified OAuth provenance was accepted by both
        // employee MCP and WebUI/control bridges. Preserve that behavior only
        // inside the explicit migration window.
        control: true,
    })
}
fn verify_legacy(bearer: &str) -> Option<Identity> {
    let until = env::var("MORROWS_OAUTH_LEGACY_UNTIL")
        .ok()?
        .parse::<i64>()
        .ok()?;
    if now() > until {
        return None;
    }

    let candidates = [
        (
            "MORROWS_OAUTH_LEGACY_RUNTIME_JWT_SECRET",
            env::var("MORROWS_OAUTH_LEGACY_RUNTIME_ISSUER")
                .unwrap_or_else(|_| "https://mcp.xycdev.com/morrows/auth".into()),
            env::var("MORROWS_OAUTH_LEGACY_RUNTIME_RESOURCE")
                .unwrap_or_else(|_| "https://mcp.xycdev.com/morrows".into()),
        ),
        (
            "MORROWS_OAUTH_LEGACY_LSM_JWT_SECRET",
            env::var("MORROWS_OAUTH_LEGACY_LSM_ISSUER")
                .unwrap_or_else(|_| "https://mcp.xycdev.com".into()),
            env::var("MORROWS_OAUTH_LEGACY_LSM_RESOURCE")
                .unwrap_or_else(|_| "https://mcp.xycdev.com".into()),
        ),
    ];
    candidates.into_iter().find_map(|(key, issuer, resource)| {
        let secret = env::var(key).ok()?;
        verify_legacy_hs256(
            bearer,
            secret.trim(),
            issuer.trim_end_matches('/'),
            resource.trim_end_matches('/'),
        )
    })
}
fn error(code: &str) -> Response {
    (StatusCode::BAD_REQUEST, Json(json!({"error":code}))).into_response()
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
async fn update<T: Send>(
    store: &Store,
    f: impl FnOnce(&mut Registry) -> T + Send,
) -> Result<T, DomainError> {
    store
        .update_oauth(|raw| {
            let mut r: Registry = serde_json::from_value(raw.clone())
                .map_err(|e| DomainError::Storage(e.to_string()))?;
            let n = now();
            r.codes.retain(|_, c| c.expires > n);
            r.prompts.retain(|_, prompt| prompt.expires > n);
            r.grants.retain(|_, g| g.expires > n);
            // Keep spent refresh hashes until the family's absolute expiry for replay detection.
            r.tokens
                .retain(|_, t| r.grants.contains_key(&t.family) && (t.refresh || t.expires > n));
            let result = f(&mut r);
            *raw = serde_json::to_value(r).map_err(|e| DomainError::Storage(e.to_string()))?;
            Ok(result)
        })
        .await?
}
pub async fn verify(store: &Store, bearer: &str) -> Option<Identity> {
    let current = update(store, |r| {
        let t = r.tokens.get(&hash(bearer))?;
        let g = r.grants.get(&t.family)?;
        if t.refresh || t.used || t.expires <= now() || g.revoked || g.expires <= now() {
            return None;
        }
        let c = r.clients.get(&g.client)?;
        Some(Identity {
            client_id: g.client.clone(),
            client_name: c.name.clone(),
            control: g.scope.split_whitespace().any(|s| s == "morrows:control"),
        })
    })
    .await
    .ok()
    .flatten();
    current.or_else(|| verify_legacy(bearer))
}
pub fn routes(state: OAuthState) -> Router {
    Router::new()
        .route(
            "/.well-known/oauth-protected-resource/morrows",
            get(resource_metadata),
        )
        .route(
            "/.well-known/oauth-authorization-server/morrows/auth",
            get(server_metadata),
        )
        .route(
            "/morrows/auth/.well-known/oauth-authorization-server",
            get(server_metadata),
        )
        .route("/morrows/auth/oauth/register", post(register))
        .route("/morrows/auth/oauth/authorize", get(authorize_get))
        .route(
            "/morrows/auth/oauth/authorize/approve",
            post(authorize_approve),
        )
        .route("/morrows/auth/oauth/consent.js", get(consent_js))
        .route("/morrows/auth/oauth/token", post(token))
        .route("/morrows/auth/oauth/revoke", post(revoke))
        .with_state(state)
        .layer(middleware::from_fn(no_store))
}
async fn no_store(request: Request<Body>, next: middleware::Next) -> Response {
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("cache-control", HeaderValue::from_static("no-store"));
    response
        .headers_mut()
        .insert("pragma", HeaderValue::from_static("no-cache"));
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; connect-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    );
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}
async fn resource_metadata(State(s): State<OAuthState>) -> Json<Value> {
    Json(
        json!({"resource":s.resource,"authorization_servers":[s.issuer],"bearer_methods_supported":["header"],"scopes_supported":["morrows"]}),
    )
}
async fn server_metadata(State(s): State<OAuthState>) -> Json<Value> {
    Json(
        json!({"issuer":s.issuer,"authorization_endpoint":format!("{}/oauth/authorize",s.issuer),"token_endpoint":format!("{}/oauth/token",s.issuer),"registration_endpoint":format!("{}/oauth/register",s.issuer),"revocation_endpoint":format!("{}/oauth/revoke",s.issuer),"response_types_supported":["code"],"grant_types_supported":["authorization_code","refresh_token"],"code_challenge_methods_supported":["S256"],"token_endpoint_auth_methods_supported":["none"],"revocation_endpoint_auth_methods_supported":["none"],"scopes_supported":["morrows", "morrows:control"]}),
    )
}
#[derive(Deserialize)]
struct Registration {
    redirect_uris: Vec<String>,
    #[serde(default)]
    client_name: String,
    token_endpoint_auth_method: Option<String>,
    grant_types: Option<Vec<String>>,
    response_types: Option<Vec<String>>,
}
fn valid_redirect(raw: &str) -> bool {
    let Ok(u) = reqwest::Url::parse(raw) else {
        return false;
    };
    u.fragment().is_none()
        && u.username().is_empty()
        && u.password().is_none()
        && u.host_str().is_some()
        && (u.scheme() == "https"
            || (u.scheme() == "http"
                && matches!(u.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"))))
}
async fn register(
    State(s): State<OAuthState>,
    Json(input): Json<Registration>,
) -> Result<Response, ApiError> {
    if input.redirect_uris.is_empty()
        || input.redirect_uris.len() > 10
        || input
            .redirect_uris
            .iter()
            .any(|u| u.len() > 2048 || !valid_redirect(u))
    {
        return Ok(error("invalid_redirect_uri"));
    }
    if input.client_name.len() > 256
        || input.client_name.chars().any(char::is_control)
        || input
            .token_endpoint_auth_method
            .as_deref()
            .is_some_and(|m| m != "none")
        || input.grant_types.as_ref().is_some_and(|v| {
            v.iter()
                .any(|g| g != "authorization_code" && g != "refresh_token")
        })
        || input
            .response_types
            .as_ref()
            .is_some_and(|v| v != &["code"])
    {
        return Ok(error("invalid_client_metadata"));
    }
    Ok(update(&s.store, |r| {
        if r.clients.len() >= 10000 { return error("temporarily_unavailable"); }
        // Never deduplicate by untrusted registration metadata: each profile is independent.
        let id = secret();
        r.clients.insert(id.clone(), Client { name: input.client_name.clone(), redirects: input.redirect_uris.clone() });
        (StatusCode::CREATED, Json(json!({"client_id":id,"client_id_issued_at":now(),"client_name":input.client_name,"redirect_uris":input.redirect_uris,"token_endpoint_auth_method":"none","grant_types":["authorization_code","refresh_token"],"response_types":["code"]}))).into_response()
    }).await?)
}
#[derive(Deserialize, Serialize)]
struct Authorization {
    client_id: String,
    redirect_uri: String,
    response_type: String,
    code_challenge: String,
    code_challenge_method: String,
    resource: Option<String>,
    scope: Option<String>,
    state: Option<String>,
    csrf: Option<String>,
}
fn approval_binding(a: &Authorization) -> String {
    hash(
        &json!([
            a.client_id,
            a.redirect_uri,
            a.response_type,
            a.code_challenge,
            a.code_challenge_method,
            a.resource,
            a.scope,
            a.state
        ])
        .to_string(),
    )
}
fn requested_scope(scope: Option<&str>) -> Option<String> {
    let mut scopes: Vec<_> = scope.unwrap_or("morrows").split_whitespace().collect();
    if scopes.is_empty()
        || scopes
            .iter()
            .any(|s| !matches!(*s, "morrows" | "morrows:control"))
    {
        return None;
    }
    scopes.sort_unstable();
    scopes.dedup();
    Some(scopes.join(" "))
}
fn valid_authorization(s: &OAuthState, r: &Registry, a: &Authorization) -> bool {
    r.clients
        .get(&a.client_id)
        .is_some_and(|c| c.redirects.contains(&a.redirect_uri))
        && a.response_type == "code"
        && a.code_challenge_method == "S256"
        && a.code_challenge.len() == 43
        && a.code_challenge
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        && a.resource.as_ref().is_none_or(|v| v == &s.resource)
        && requested_scope(a.scope.as_deref()).is_some()
        && a.state.as_ref().is_none_or(|v| v.len() <= 4096)
}
async fn consent_js() -> Response {
    (
        [(CONTENT_TYPE, "application/javascript; charset=utf-8")],
        CONSENT_JS,
    )
        .into_response()
}

fn approval_role_allows(identity: &crate::operator_auth::OperatorIdentity, scope: &str) -> bool {
    let role = identity.0.get("role").and_then(Value::as_str).unwrap_or("");
    let needs_admin = scope
        .split_whitespace()
        .any(|value| value == "morrows:control");
    if needs_admin {
        role == "admin"
    } else {
        matches!(role, "operator" | "admin")
    }
}

async fn authorize_get(
    State(s): State<OAuthState>,
    Query(a): Query<Authorization>,
) -> Result<Response, ApiError> {
    Ok(update(&s.store, |r| {
        if !valid_authorization(&s, r, &a) {
            return error("invalid_request");
        }
        if r.prompts.len() >= 1000 {
            return error("temporarily_unavailable");
        }
        let csrf = secret();
        r.prompts.insert(
            hash(&csrf),
            Prompt {
                expires: now() + 300,
                binding: approval_binding(&a),
            },
        );
        let mut fields = String::new();
        for (key, value) in serde_json::to_value(&a).unwrap().as_object().unwrap() {
            if let Some(value) = value.as_str() {
                if key != "csrf" {
                    fields.push_str(&format!(
                        "<input type=\"hidden\" name=\"{}\" value=\"{}\">",
                        escape(key),
                        escape(value)
                    ));
                }
            }
        }
        let scope = requested_scope(a.scope.as_deref()).unwrap();
        let needs_admin = scope
            .split_whitespace()
            .any(|value| value == "morrows:control");
        let name = &r.clients[&a.client_id].name;
        let role = if needs_admin { "admin" } else { "operator" };
        let mut response = Html(format!(
            "<!doctype html><meta charset=\"utf-8\"><title>Morrows authorization</title>\
             <h1>Authorize Morrows</h1>\
             <p>Allow {} to access Morrows?</p>\
             <p>Permissions: {}</p>\
             <p>Redirect: {}</p>\
             <p>This approval reuses your Morrows {} login. No separate OAuth secret is required.</p>\
             <form id=\"oauth-consent\" action=\"/morrows/auth/oauth/authorize/approve\" data-requires-admin=\"{}\">\
             {}<input type=\"hidden\" name=\"csrf\" value=\"{}\">\
             <button id=\"oauth-approve\" type=\"button\">Authorize with Morrows</button></form>\
             <p id=\"oauth-status\" role=\"status\"></p><pre id=\"oauth-command\"></pre>\
             <script src=\"/morrows/auth/oauth/consent.js\" defer></script>",
            escape(name),
            escape(&scope),
            escape(&a.redirect_uri),
            role,
            needs_admin,
            fields,
            csrf
        ))
        .into_response();
        response.headers_mut().insert(
            "set-cookie",
            HeaderValue::from_str(&format!(
                "morrows_oauth_csrf={csrf}; Path=/morrows/auth; Secure; HttpOnly; SameSite=Strict; Max-Age=300"
            ))
            .unwrap(),
        );
        response
    })
    .await?)
}

async fn authorize_approve(
    State(s): State<OAuthState>,
    headers: HeaderMap,
    axum::Extension(identity): axum::Extension<crate::operator_auth::OperatorIdentity>,
    Form(a): Form<Authorization>,
) -> Result<Response, ApiError> {
    Ok(update(&s.store, |r| {
        if !valid_authorization(&s, r, &a) {
            return error("invalid_request");
        }
        let csrf = a.csrf.as_deref().unwrap_or("");
        let cookie_ok = headers
            .get("cookie")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|cookies| {
                cookies
                    .split(';')
                    .any(|part| part.trim() == format!("morrows_oauth_csrf={csrf}"))
            });
        if !cookie_ok
            || !r
                .prompts
                .get(&hash(csrf))
                .is_some_and(|prompt| prompt.binding == approval_binding(&a))
        {
            return error("invalid_request");
        }
        let scope = requested_scope(a.scope.as_deref()).unwrap();
        if !approval_role_allows(&identity, &scope) {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "error": if scope.split_whitespace().any(|value| value == "morrows:control") {
                        "Morrows admin approval required"
                    } else {
                        "Morrows operator approval required"
                    }
                })),
            )
                .into_response();
        }

        // Consume the prompt only after identity and role checks pass. A stale or
        // insufficient operator session can therefore sign in and retry safely.
        r.prompts.remove(&hash(csrf));
        let code = secret();
        r.codes.insert(
            hash(&code),
            Code {
                scope,
                client: a.client_id,
                redirect: a.redirect_uri.clone(),
                challenge: a.code_challenge,
                expires: now() + 300,
            },
        );
        let mut target = reqwest::Url::parse(&a.redirect_uri).unwrap();
        target.query_pairs_mut().append_pair("code", &code);
        if let Some(state) = a.state {
            target.query_pairs_mut().append_pair("state", &state);
        }
        Json(json!({"redirect_to": target.as_str()})).into_response()
    })
    .await?)
}
#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    client_id: String,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
    refresh_token: Option<String>,
    resource: Option<String>,
    scope: Option<String>,
}
fn issue(r: &mut Registry, family: String) -> Response {
    let access = secret();
    let refresh = secret();
    let n = now();
    r.tokens.insert(
        hash(&access),
        Token {
            family: family.clone(),
            expires: n + ACCESS_TTL,
            refresh: false,
            used: false,
        },
    );
    let expires = r.grants[&family].expires;
    r.tokens.insert(
        hash(&refresh),
        Token {
            family: family.clone(),
            expires,
            refresh: true,
            used: false,
        },
    );
    Json(json!({"access_token":access,"token_type":"Bearer","expires_in":ACCESS_TTL,"refresh_token":refresh,"scope":r.grants[&family].scope})).into_response()
}
async fn token(
    State(s): State<OAuthState>,
    Form(a): Form<TokenRequest>,
) -> Result<Response, ApiError> {
    Ok(update(&s.store, |r| {
        if !r.clients.contains_key(&a.client_id) {
            return error("invalid_client");
        }
        if a.resource.as_ref().is_some_and(|v| v != &s.resource) {
            return error("invalid_target");
        }
        if requested_scope(a.scope.as_deref()).is_none() {
            return error("invalid_scope");
        }
        match a.grant_type.as_str() {
            "authorization_code" => {
                let key = hash(a.code.as_deref().unwrap_or(""));
                let Some(c) = r.codes.get(&key) else {
                    return error("invalid_grant");
                };
                let verifier = a.code_verifier.as_deref().unwrap_or("");
                if c.client != a.client_id
                    || Some(&c.redirect) != a.redirect_uri.as_ref()
                    || !(43..=128).contains(&verifier.len())
                    || !verifier
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-._~".contains(&b))
                    || hash(verifier) != c.challenge
                {
                    return error("invalid_grant");
                }
                let scope = c.scope.clone();
                if a.scope.is_some() && requested_scope(a.scope.as_deref()).as_ref() != Some(&scope)
                {
                    return error("invalid_scope");
                }
                r.codes.remove(&key);
                let family = secret();
                r.grants.insert(
                    family.clone(),
                    Grant {
                        scope,
                        client: a.client_id,
                        expires: now() + FAMILY_TTL,
                        revoked: false,
                    },
                );
                issue(r, family)
            }
            "refresh_token" => {
                let key = hash(a.refresh_token.as_deref().unwrap_or(""));
                let Some(t) = r.tokens.get_mut(&key) else {
                    return error("invalid_grant");
                };
                let Some(g) = r.grants.get_mut(&t.family) else {
                    return error("invalid_grant");
                };
                if !t.refresh || g.client != a.client_id || g.revoked || t.expires <= now() {
                    return error("invalid_grant");
                }
                if a.scope.is_some()
                    && requested_scope(a.scope.as_deref()).as_ref() != Some(&g.scope)
                {
                    return error("invalid_scope");
                }
                if t.used {
                    g.revoked = true;
                    return error("invalid_grant");
                }
                t.used = true;
                let family = t.family.clone();
                issue(r, family)
            }
            _ => error("unsupported_grant_type"),
        }
    })
    .await?)
}
#[derive(Deserialize)]
struct Revocation {
    client_id: String,
    token: String,
}
async fn revoke(
    State(s): State<OAuthState>,
    Form(a): Form<Revocation>,
) -> Result<Response, ApiError> {
    update(&s.store, |r| {
        if let Some(t) = r.tokens.get(&hash(&a.token)) {
            if let Some(g) = r.grants.get_mut(&t.family) {
                if g.client == a.client_id {
                    g.revoked = true;
                }
            }
        }
    })
    .await?;
    Ok(StatusCode::OK.into_response())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tower::ServiceExt;
    pub async fn test_bearer(store: &Store, client: &str) -> String {
        let response = update(store, |r| {
            r.clients.insert(
                client.into(),
                Client {
                    name: "Codex".into(),
                    redirects: vec!["http://localhost:1455/callback".into()],
                },
            );
            let family = secret();
            r.grants.insert(
                family.clone(),
                Grant {
                    scope: "morrows".into(),
                    client: client.into(),
                    expires: now() + FAMILY_TTL,
                    revoked: false,
                },
            );
            issue(r, family)
        })
        .await
        .unwrap();
        body(response).await["access_token"]
            .as_str()
            .unwrap()
            .into()
    }
    async fn body(response: Response) -> Value {
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap()
    }
    fn state(store: Store) -> OAuthState {
        OAuthState {
            store,
            resource: RESOURCE.into(),
            issuer: format!("{RESOURCE}/auth"),
        }
    }
    async fn post(app: &Router, endpoint: &str, fields: &[(&str, &str)]) -> Response {
        let mut url = reqwest::Url::parse("https://unused/").unwrap();
        url.query_pairs_mut().extend_pairs(fields.iter().copied());
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(endpoint)
                    .header("content-type", "application/x-www-form-urlencoded")
                    .body(Body::from(url.query().unwrap().to_owned()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    async fn code(s: &OAuthState, client: &str, verifier: &str) -> String {
        let code = secret();
        update(&s.store, |r| {
            r.clients.insert(
                client.into(),
                Client {
                    name: "Codex".into(),
                    redirects: vec!["http://localhost:1455/callback".into()],
                },
            );
            r.codes.insert(
                hash(&code),
                Code {
                    scope: "morrows".into(),
                    client: client.into(),
                    redirect: "http://localhost:1455/callback".into(),
                    challenge: hash(verifier),
                    expires: now() + 300,
                },
            );
        })
        .await
        .unwrap();
        code
    }
    async fn exchange(app: &Router, code: &str, client: &str, verifier: &str) -> Response {
        post(
            app,
            "/morrows/auth/oauth/token",
            &[
                ("grant_type", "authorization_code"),
                ("client_id", client),
                ("code", code),
                ("redirect_uri", "http://localhost:1455/callback"),
                ("code_verifier", verifier),
                ("resource", RESOURCE),
            ],
        )
        .await
    }
    async fn refresh(app: &Router, token: &str, client: &str) -> Response {
        post(
            app,
            "/morrows/auth/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("client_id", client),
                ("refresh_token", token),
                ("resource", RESOURCE),
            ],
        )
        .await
    }
    fn legacy_token(secret: &str, issuer: &str, resource: &str, client: &str, exp: i64) -> String {
        let header = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "alg": "HS256", "typ": "JWT"
            }))
            .unwrap(),
        );
        let claims = URL_SAFE_NO_PAD.encode(
            serde_json::to_vec(&json!({
                "iss": issuer,
                "aud": resource,
                "iat": now() - 10,
                "exp": exp,
                "client_id": client,
                "scope": "shell:read"
            }))
            .unwrap(),
        );
        let input = format!("{header}.{claims}");
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(input.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        format!("{input}.{signature}")
    }

    #[test]
    fn bounded_legacy_hs256_verifier_checks_signature_issuer_audience_and_expiry() {
        let secret = "legacy-secret-that-is-definitely-at-least-32-bytes";
        let issuer = "https://mcp.xycdev.com/morrows/auth";
        let resource = RESOURCE;
        let token = legacy_token(secret, issuer, resource, "old-chatgpt", now() + 60);
        let identity = verify_legacy_hs256(&token, secret, issuer, resource).unwrap();
        assert_eq!(identity.client_id, "old-chatgpt");
        assert!(identity.control);
        assert!(
            verify_legacy_hs256(
                &token,
                "wrong-secret-that-is-at-least-32-bytes",
                issuer,
                resource
            )
            .is_none()
        );
        assert!(verify_legacy_hs256(&token, secret, "https://wrong.example", resource).is_none());
        assert!(verify_legacy_hs256(&token, secret, issuer, "https://wrong.example").is_none());
        let expired = legacy_token(secret, issuer, resource, "old-chatgpt", now() - 1);
        assert!(verify_legacy_hs256(&expired, secret, issuer, resource).is_none());
    }

    #[tokio::test]
    async fn pkce_single_use_rotation_replay_revokes_family() {
        let s = state(Store::connect("sqlite::memory:").await.unwrap());
        let app = routes(s.clone());
        let verifier = "a".repeat(43);
        let code = code(&s, "profile-a", &verifier).await;
        assert_eq!(
            exchange(&app, &code, "profile-a", &"b".repeat(43))
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
        let initial = body(exchange(&app, &code, "profile-a", &verifier).await).await;
        assert_eq!(
            exchange(&app, &code, "profile-a", &verifier).await.status(),
            StatusCode::BAD_REQUEST
        );
        let access = initial["access_token"].as_str().unwrap();
        let old = initial["refresh_token"].as_str().unwrap();
        assert_eq!(
            verify(&s.store, access).await.unwrap().client_id,
            "profile-a"
        );
        assert!(verify(&s.store, old).await.is_none());
        let rotated = body(refresh(&app, old, "profile-a").await).await;
        assert_ne!(rotated["refresh_token"], initial["refresh_token"]);
        assert!(
            verify(&s.store, rotated["access_token"].as_str().unwrap())
                .await
                .is_some()
        );
        assert_eq!(
            refresh(&app, old, "profile-a").await.status(),
            StatusCode::BAD_REQUEST
        );
        assert!(verify(&s.store, access).await.is_none());
        assert!(
            verify(&s.store, rotated["access_token"].as_str().unwrap())
                .await
                .is_none()
        );
        assert_eq!(
            refresh(
                &app,
                rotated["refresh_token"].as_str().unwrap(),
                "profile-a"
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
    }
    #[tokio::test]
    async fn separate_clients_resource_binding_and_revocation() {
        let s = state(Store::connect("sqlite::memory:").await.unwrap());
        let app = routes(s.clone());
        let v = "v".repeat(43);
        let a = code(&s, "a", &v).await;
        let b = code(&s, "b", &v).await;
        assert_eq!(
            exchange(&app, &a, "b", &v).await.status(),
            StatusCode::BAD_REQUEST
        );
        let a = body(exchange(&app, &a, "a", &v).await).await;
        let b = body(exchange(&app, &b, "b", &v).await).await;
        let rt = a["refresh_token"].as_str().unwrap();
        assert_eq!(
            refresh(&app, rt, "b").await.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post(
                &app,
                "/morrows/auth/oauth/token",
                &[
                    ("grant_type", "refresh_token"),
                    ("client_id", "a"),
                    ("refresh_token", rt),
                    ("resource", "https://wrong.example/")
                ]
            )
            .await
            .status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            post(
                &app,
                "/morrows/auth/oauth/revoke",
                &[("client_id", "a"), ("token", rt)]
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert!(
            verify(&s.store, a["access_token"].as_str().unwrap())
                .await
                .is_none()
        );
        assert!(
            verify(&s.store, b["access_token"].as_str().unwrap())
                .await
                .is_some()
        );
    }
    #[tokio::test]
    async fn persisted_clients_codes_and_refresh_survive_reopen_and_concurrent_redemption() {
        let path = std::env::temp_dir().join(format!("morrows-oauth-{}.sqlite", Uuid::new_v4()));
        let url = format!("sqlite://{}", path.display());
        let s = state(Store::connect(&url).await.unwrap());
        let v = "z".repeat(43);
        let c = code(&s, "persistent", &v).await;
        drop(s);
        let s = state(Store::connect(&url).await.unwrap());
        let app = routes(s.clone());
        let other = routes(state(Store::connect(&url).await.unwrap()));
        let (a, b) = tokio::join!(
            exchange(&app, &c, "persistent", &v),
            exchange(&other, &c, "persistent", &v)
        );
        assert_ne!(a.status(), b.status());
        let issued = body(if a.status() == StatusCode::OK { a } else { b }).await;
        drop(app);
        drop(other);
        drop(s);
        let s = state(Store::connect(&url).await.unwrap());
        let app = routes(s.clone());
        assert_eq!(
            verify(&s.store, issued["access_token"].as_str().unwrap())
                .await
                .unwrap()
                .client_id,
            "persistent"
        );
        let rt = issued["refresh_token"].as_str().unwrap();
        let (a, b) = tokio::join!(
            refresh(&app, rt, "persistent"),
            refresh(&app, rt, "persistent")
        );
        assert_ne!(a.status(), b.status());
        let rotated = body(if a.status() == StatusCode::OK { a } else { b }).await;
        assert!(
            verify(&s.store, rotated["access_token"].as_str().unwrap())
                .await
                .is_none()
        );
        drop(app);
        drop(s);
        let _ = std::fs::remove_file(path);
    }
    #[tokio::test]
    async fn expiration_and_no_plaintext_secrets_in_storage() {
        let s = state(Store::connect("sqlite::memory:").await.unwrap());
        let app = routes(s.clone());
        let v = "x".repeat(43);
        let c = code(&s, "a", &v).await;
        let issued = body(exchange(&app, &c, "a", &v).await).await;
        let access = issued["access_token"].as_str().unwrap();
        let rt = issued["refresh_token"].as_str().unwrap();
        s.store
            .update_oauth(|raw| {
                let text = raw.to_string();
                assert!(!text.contains(access));
                assert!(!text.contains(rt));
                assert!(!text.contains(&c));
            })
            .await
            .unwrap();
        update(&s.store, |r| {
            r.tokens.get_mut(&hash(access)).unwrap().expires = now() - 1;
        })
        .await
        .unwrap();
        assert!(verify(&s.store, access).await.is_none());
        assert_eq!(refresh(&app, rt, "a").await.status(), StatusCode::OK);
        update(&s.store, |r| {
            for grant in r.grants.values_mut() {
                grant.expires = now() - 1;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            refresh(&app, rt, "a").await.status(),
            StatusCode::BAD_REQUEST
        );
    }
    #[tokio::test]
    async fn mcp_scope_cannot_escalate_to_control_on_refresh() {
        let s = state(Store::connect("sqlite::memory:").await.unwrap());
        let app = routes(s.clone());
        let v = "a".repeat(43);
        let c = code(&s, "mcp-client", &v).await;
        let issued = body(exchange(&app, &c, "mcp-client", &v).await).await;
        let access = issued["access_token"].as_str().unwrap();
        assert!(!verify(&s.store, access).await.unwrap().control);
        let control_app = Router::new()
            .route("/api/tasks", get(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(
                crate::operator_auth::OperatorAuthState::new(s.store.clone(), true, None),
                crate::operator_auth::authenticate_operator_requests,
            ));
        let request = || {
            Request::builder()
                .uri("/api/tasks")
                .header("authorization", format!("Bearer {access}"))
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            control_app
                .clone()
                .oneshot(request())
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let response = post(
            &app,
            "/morrows/auth/oauth/token",
            &[
                ("grant_type", "refresh_token"),
                ("client_id", "mcp-client"),
                ("refresh_token", issued["refresh_token"].as_str().unwrap()),
                ("scope", "morrows:control"),
            ],
        )
        .await;
        assert_eq!(body(response).await["error"], "invalid_scope");
        // Only an explicitly approved control grant can enter operator routes.
        update(&s.store, |r| {
            for g in r.grants.values_mut() {
                g.scope = "morrows morrows:control".into();
            }
        })
        .await
        .unwrap();
        assert_eq!(
            control_app.oneshot(request()).await.unwrap().status(),
            StatusCode::OK
        );
    }

    #[tokio::test]
    async fn discovery_registration_consent_and_operator_approval() {
        let s = state(Store::connect("sqlite::memory:").await.unwrap());
        let app = routes(s.clone()).layer(middleware::from_fn_with_state(
            crate::operator_auth::OperatorAuthState::new(s.store.clone(), true, None),
            crate::operator_auth::authenticate_operator_requests,
        ));
        let metadata = body(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/.well-known/oauth-authorization-server/morrows/auth")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(metadata["issuer"], format!("{RESOURCE}/auth"));
        assert_eq!(metadata["grant_types_supported"][1], "refresh_token");
        let register = || {
            Request::builder()
                .method("POST")
                .uri("/morrows/auth/oauth/register")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"client_name":"Codex","redirect_uris":["http://localhost:1455/callback"]}"#,
                ))
                .unwrap()
        };
        let a = body(app.clone().oneshot(register()).await.unwrap()).await;
        let b = body(app.clone().oneshot(register()).await.unwrap()).await;
        assert_ne!(a["client_id"], b["client_id"]);
        let client = a["client_id"].as_str().unwrap();
        let verifier = "y".repeat(43);
        let challenge = hash(&verifier);
        let mut url = reqwest::Url::parse("https://unused/morrows/auth/oauth/authorize").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("client_id", client),
            ("redirect_uri", "http://localhost:1455/callback"),
            ("response_type", "code"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("resource", RESOURCE),
            ("state", "state-value"),
        ]);
        let page = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{}?{}", url.path(), url.query().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let cookie = page.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let page_text = String::from_utf8(
            axum::body::to_bytes(page.into_body(), usize::MAX)
                .await
                .unwrap()
                .to_vec(),
        )
        .unwrap();
        assert!(page_text.contains("reuses your Morrows operator login"));
        assert!(!page_text.contains("approval secret"));
        let csrf = cookie.split_once('=').unwrap().1;
        url.query_pairs_mut().append_pair("csrf", csrf);
        let form = url.query().unwrap().to_owned();
        let make = |cookie: &str, token: Option<&str>| {
            let mut request = Request::builder()
                .method("POST")
                .uri("/morrows/auth/oauth/authorize/approve")
                .header("cookie", cookie)
                .header("content-type", "application/x-www-form-urlencoded");
            if let Some(token) = token {
                request = request.header("authorization", format!("Bearer {token}"));
            }
            request.body(Body::from(form.clone())).unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(make(&cookie, None))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let login = s
            .store
            .request_operator_login("OAuth consent")
            .await
            .unwrap();
        s.store
            .approve_operator_login(&login.code, "operator", 600)
            .await
            .unwrap();
        assert_eq!(
            app.clone()
                .oneshot(make("", Some(&login.token)))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        let approved = app
            .clone()
            .oneshot(make(&cookie, Some(&login.token)))
            .await
            .unwrap();
        assert_eq!(approved.status(), StatusCode::OK);
        let approved = body(approved).await;
        let location = reqwest::Url::parse(approved["redirect_to"].as_str().unwrap()).unwrap();
        let fields: BTreeMap<_, _> = location.query_pairs().into_owned().collect();
        assert_eq!(fields["state"], "state-value");
        assert_eq!(
            exchange(&app, &fields["code"], client, &verifier)
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(
            app.clone()
                .oneshot(make(&cookie, Some(&login.token)))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );
        assert!(!valid_redirect("https://example.com/callback#fragment"));
        assert!(!valid_redirect("http://evil.example/callback"));
    }

    #[tokio::test]
    async fn control_scope_requires_admin_without_consuming_consent() {
        let s = state(Store::connect("sqlite::memory:").await.unwrap());
        update(&s.store, |r| {
            r.clients.insert(
                "control-client".into(),
                Client {
                    name: "Morrows WebUI".into(),
                    redirects: vec!["http://localhost:1455/callback".into()],
                },
            );
        })
        .await
        .unwrap();
        let app = routes(s.clone()).layer(middleware::from_fn_with_state(
            crate::operator_auth::OperatorAuthState::new(s.store.clone(), true, None),
            crate::operator_auth::authenticate_operator_requests,
        ));
        let verifier = "c".repeat(43);
        let challenge = hash(&verifier);
        let mut url = reqwest::Url::parse("https://unused/morrows/auth/oauth/authorize").unwrap();
        url.query_pairs_mut().extend_pairs([
            ("client_id", "control-client"),
            ("redirect_uri", "http://localhost:1455/callback"),
            ("response_type", "code"),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("resource", RESOURCE),
            ("scope", "morrows morrows:control"),
        ]);
        let page = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("{}?{}", url.path(), url.query().unwrap()))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let cookie = page.headers()["set-cookie"]
            .to_str()
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .to_owned();
        let csrf = cookie.split_once('=').unwrap().1;
        url.query_pairs_mut().append_pair("csrf", csrf);
        let form = url.query().unwrap().to_owned();
        let make = |token: &str| {
            Request::builder()
                .method("POST")
                .uri("/morrows/auth/oauth/authorize/approve")
                .header("cookie", &cookie)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/x-www-form-urlencoded")
                .body(Body::from(form.clone()))
                .unwrap()
        };
        let operator = s.store.request_operator_login("operator").await.unwrap();
        s.store
            .approve_operator_login(&operator.code, "operator", 600)
            .await
            .unwrap();
        assert_eq!(
            app.clone()
                .oneshot(make(&operator.token))
                .await
                .unwrap()
                .status(),
            StatusCode::FORBIDDEN
        );
        let admin = s.store.request_operator_login("admin").await.unwrap();
        s.store
            .approve_operator_login(&admin.code, "admin", 600)
            .await
            .unwrap();
        assert_eq!(
            app.clone()
                .oneshot(make(&admin.token))
                .await
                .unwrap()
                .status(),
            StatusCode::OK
        );
    }
}
