"""Exercise the public split between the MCP audience and OAuth route prefix."""

from urllib.parse import parse_qs, urlsplit

from starlette.applications import Starlette
from starlette.routing import Mount, Route
from starlette.testclient import TestClient
from test_morrows_bridge import _FakeAsyncClient

from morrow_runtime import morrows_bridge, oauth
from morrow_runtime.auth import AuthMiddleware
from morrow_runtime.settings import get_settings


def test_split_issuer_resource_discovery_and_pkce(tmp_path, monkeypatch):
    base = "http://testserver"
    issuer = base + "/morrows/auth"
    resource = base + "/morrows"
    for key, value in {
        "WORKSPACE_ROOT": str(tmp_path),
        "STATE_DIR": str(tmp_path / "state"),
        "AUTH_MODE": "oauth",
        "AUTH_BYPASS_LOCALHOST": "false",
        "PUBLIC_BASE_URL": base + "/morrows/ui/runtime",
        "OAUTH_ISSUER": issuer,
        "OAUTH_RESOURCE": resource,
        "OAUTH_JWT_SECRET": "test-morrows-resource-secret-over-thirty-two-bytes",
        "OAUTH_ADMIN_PIN": "test-only-pin",
    }.items():
        monkeypatch.setenv("LOCAL_SHELL_MCP_" + key, value)
    get_settings.cache_clear()
    oauth._CLIENTS.clear()
    oauth._CODES.clear()
    oauth._PIN_FAILURES.clear()
    protected = Starlette(routes=morrows_bridge.morrows_bridge_routes())
    protected.add_middleware(AuthMiddleware)
    # Model Caddy's /morrows/auth prefix independently from the MCP resource.
    auth = Starlette(
        routes=[
            Route("/.well-known/oauth-protected-resource", oauth.oauth_protected_resource),
            Route("/.well-known/oauth-authorization-server", oauth.oauth_server_metadata),
            Route("/oauth/register", oauth.oauth_register, methods=["POST"]),
            Route("/oauth/authorize", oauth.oauth_authorize_post, methods=["POST"]),
            Route("/oauth/token", oauth.oauth_token, methods=["POST"]),
        ]
    )
    edge = Starlette(
        routes=[
            Route("/.well-known/oauth-protected-resource/morrows", oauth.oauth_protected_resource),
            Mount("/morrows/auth", auth),
            Mount("/", protected),
        ]
    )
    _FakeAsyncClient.calls = []
    monkeypatch.setattr(morrows_bridge.httpx, "AsyncClient", _FakeAsyncClient)
    with TestClient(edge, base_url=base) as client:
        rejected = client.post("/morrows", json={"jsonrpc": "2.0", "id": 1, "method": "tools/list"})
        assert rejected.status_code == 401
        challenge = rejected.headers["www-authenticate"]
        metadata_url = challenge.split('resource_metadata="', 1)[1].split('"', 1)[0]
        assert metadata_url == base + "/.well-known/oauth-protected-resource/morrows"
        metadata = client.get(metadata_url)
        assert metadata.status_code == 200
        assert metadata.json()["resource"] == resource
        assert metadata.json()["authorization_servers"] == [issuer]
        server = client.get(issuer + "/.well-known/oauth-authorization-server").json()
        assert server["issuer"] == issuer
        registered = client.post(
            server["registration_endpoint"],
            json={
                "client_name": "resource-regression",
                "redirect_uris": ["https://client.test/callback"],
            },
        )
        assert registered.status_code == 201
        client_id = registered.json()["client_id"]
        # RFC 7636's fixed PKCE vector keeps this test focused on URI binding.
        verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"
        params = {
            "response_type": "code",
            "client_id": client_id,
            "redirect_uri": "https://client.test/callback",
            "code_challenge": "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            "code_challenge_method": "S256",
            "pin": "test-only-pin",
            "resource": resource,
        }
        authorized = client.post(
            server["authorization_endpoint"], data=params, follow_redirects=False
        )
        assert authorized.status_code in (302, 303)
        code = parse_qs(urlsplit(authorized.headers["location"]).query)["code"][0]
        exchanged = client.post(
            server["token_endpoint"],
            data={
                "grant_type": "authorization_code",
                "client_id": client_id,
                "code": code,
                "redirect_uri": params["redirect_uri"],
                "code_verifier": verifier,
                "resource": resource,
            },
        )
        assert exchanged.status_code == 200, exchanged.text
        token = exchanged.json()["access_token"]
        claims = oauth.validate_bearer_token(token)
        assert claims["aud"] == resource
        assert claims["iss"] == issuer
        result = client.post(
            "/morrows",
            headers={"Authorization": f"Bearer {token}"},
            json={"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        )
        assert result.status_code == 200
        assert len(_FakeAsyncClient.calls) == 1
        invalid = client.post(server["authorization_endpoint"], data={**params, "resource": issuer})
        assert "resource does not match this protected resource" in invalid.text
        assert "location" not in invalid.headers
        wrong_audience = oauth.issue_access_token(
            client_id=client_id, scope="shell:read", resource=issuer, issuer=issuer
        )
        denied = client.post("/morrows", headers={"Authorization": f"Bearer {wrong_audience}"})
        assert denied.status_code == 401
        assert len(_FakeAsyncClient.calls) == 1


def test_runtime_serves_standard_resource_path_and_rejects_other_resources(tmp_path, monkeypatch):
    from morrow_runtime.main import _build_mcp_http_app
    from morrow_runtime.tools import build_mcp

    for key, value in {
        "WORKSPACE_ROOT": str(tmp_path),
        "STATE_DIR": str(tmp_path / "state"),
        "AUTH_MODE": "oauth",
        "REMOTE_ENABLED": "false",
        "PUBLIC_BASE_URL": "http://testserver/morrows/ui/runtime",
        "OAUTH_RESOURCE": "http://testserver/morrows",
        "OAUTH_ISSUER": "http://testserver/morrows/auth",
        "OAUTH_JWT_SECRET": "resource-route-test-secret-at-least-thirty-two-bytes",
    }.items():
        monkeypatch.setenv("LOCAL_SHELL_MCP_" + key, value)
    get_settings.cache_clear()
    with TestClient(_build_mcp_http_app(build_mcp())) as client:
        path = "/.well-known/oauth-protected-resource/morrows"
        response = client.get(path)
        assert response.status_code == 200
        assert response.json()["resource"] == "http://testserver/morrows"
        assert client.get("/.well-known/oauth-protected-resource/unrelated").status_code == 404
        # Keep the issuer-mounted old alias compatible, but advertise the same audience.
        assert client.get("/.well-known/oauth-protected-resource").json() == response.json()
