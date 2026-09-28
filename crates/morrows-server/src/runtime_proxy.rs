use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{OriginalUri, State},
    http::{
        HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode,
        header::{CONNECTION, CONTENT_LENGTH, HOST, TRANSFER_ENCODING},
    },
    response::IntoResponse,
    routing::any,
};
use std::{env, net::IpAddr};

const MAX_PROXY_BODY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
struct RuntimeProxyState {
    client: reqwest::Client,
    origin: String,
}

pub fn router_from_env() -> anyhow::Result<Option<Router>> {
    let configured =
        env::var("MORROWS_RUNTIME_PROXY_URL").or_else(|_| env::var("MORROWS_RUNTIME_CONTROL_URL"));
    let Ok(configured) = configured else {
        return Ok(None);
    };
    let origin = configured
        .trim()
        .trim_end_matches('/')
        .trim_end_matches("/api/control")
        .to_owned();
    let parsed = reqwest::Url::parse(&origin)?;
    if parsed.scheme() != "http" {
        anyhow::bail!("MORROWS_RUNTIME_PROXY_URL must use loopback HTTP");
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("MORROWS_RUNTIME_PROXY_URL must contain a host"))?;
    let ip: IpAddr = host.parse().map_err(|_| {
        anyhow::anyhow!("MORROWS_RUNTIME_PROXY_URL must use a numeric loopback host")
    })?;
    if !ip.is_loopback() {
        anyhow::bail!("MORROWS_RUNTIME_PROXY_URL must target loopback");
    }
    if parsed.path() != "/" && !parsed.path().is_empty() {
        anyhow::bail!("MORROWS_RUNTIME_PROXY_URL must not contain a path");
    }

    let state = RuntimeProxyState {
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()?,
        origin,
    };
    Ok(Some(
        Router::new()
            .route("/", any(proxy_runtime))
            .route("/{*path}", any(proxy_runtime))
            .with_state(state),
    ))
}

fn is_public_runtime_path(path: &str) -> bool {
    matches!(
        path,
        "/mcp" | "/healthz" | "/readyz" | "/join" | "/join.ps1"
    ) || path.starts_with("/remote/")
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str().to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

async fn proxy_runtime(
    State(state): State<RuntimeProxyState>,
    OriginalUri(original_uri): OriginalUri,
    request: Request<Body>,
) -> Response<Body> {
    let original_path = original_uri.path();
    let Some(path) = original_path.strip_prefix("/runtime") else {
        return (StatusCode::NOT_FOUND, "runtime route not found").into_response();
    };
    let path = if path.is_empty() { "/" } else { path };
    if !is_public_runtime_path(path) {
        return (StatusCode::NOT_FOUND, "runtime route not found").into_response();
    }

    let target = match original_uri.query() {
        Some(query) => format!("{}{path}?{query}", state.origin),
        None => format!("{}{path}", state.origin),
    };
    let (parts, body) = request.into_parts();
    let body = match to_bytes(body, MAX_PROXY_BODY_BYTES).await {
        Ok(body) => body,
        Err(err) => {
            tracing::warn!(%err, "failed reading morrow-runtime proxy request body");
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                "runtime proxy request body is invalid or too large",
            )
                .into_response();
        }
    };

    let method = match reqwest::Method::from_bytes(parts.method.as_str().as_bytes()) {
        Ok(method) => method,
        Err(_) => return (StatusCode::BAD_REQUEST, "invalid runtime proxy method").into_response(),
    };
    let mut upstream = state.client.request(method, target);
    for (name, value) in &parts.headers {
        if *name == HOST
            || *name == CONNECTION
            || *name == CONTENT_LENGTH
            || *name == TRANSFER_ENCODING
            || is_hop_by_hop(name)
        {
            continue;
        }
        let Ok(upstream_name) = reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes())
        else {
            continue;
        };
        let Ok(upstream_value) = reqwest::header::HeaderValue::from_bytes(value.as_bytes()) else {
            continue;
        };
        upstream = upstream.header(upstream_name, upstream_value);
    }

    let upstream = match upstream.body(body).send().await {
        Ok(response) => response,
        Err(err) => {
            tracing::warn!(%err, "morrow-runtime proxy request failed");
            return (
                StatusCode::BAD_GATEWAY,
                "morrow-runtime sidecar is unavailable",
            )
                .into_response();
        }
    };
    let status =
        StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let headers = upstream.headers().clone();
    let body = match upstream.bytes().await {
        Ok(body) => body,
        Err(err) => {
            tracing::warn!(%err, "failed reading morrow-runtime proxy response");
            return (
                StatusCode::BAD_GATEWAY,
                "invalid response from morrow-runtime sidecar",
            )
                .into_response();
        }
    };

    let mut response = Response::builder().status(status);
    if let Some(response_headers) = response.headers_mut() {
        copy_response_headers(response_headers, &headers);
    }
    response
        .body(Body::from(body))
        .unwrap_or_else(|_| StatusCode::BAD_GATEWAY.into_response())
}

fn copy_response_headers(target: &mut HeaderMap, source: &reqwest::header::HeaderMap) {
    for (name, value) in source {
        let Ok(name) = HeaderName::from_bytes(name.as_str().as_bytes()) else {
            continue;
        };
        if name == CONNECTION
            || name == CONTENT_LENGTH
            || name == TRANSFER_ENCODING
            || is_hop_by_hop(&name)
        {
            continue;
        }
        let Ok(value) = HeaderValue::from_bytes(value.as_bytes()) else {
            continue;
        };
        target.append(name, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_proxy_surface_excludes_control_api() {
        for path in [
            "/mcp",
            "/healthz",
            "/readyz",
            "/join",
            "/join.ps1",
            "/remote/poll",
            "/remote/worker-bundle.tgz",
            "/remote/transfers/abc",
        ] {
            assert!(is_public_runtime_path(path), "{path}");
        }
        for path in [
            "/",
            "/api/control/sessions",
            "/ui",
            "/oauth/token",
            "/downloads/x",
        ] {
            assert!(!is_public_runtime_path(path), "{path}");
        }
    }
}
