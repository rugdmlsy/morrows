"""Browser-level Morrows OAuth persistence regression.

Start web preview first:
  cd web && npm ci && npm run build && npm run preview -- --host 127.0.0.1 --port 4179
Run:
  uv run --no-project --with playwright python web/tests/browserOAuthPersistence.smoke.py

Intercepts only preview API/OAuth endpoints; no production credentials needed.
"""
from playwright.sync_api import sync_playwright
from urllib.parse import parse_qs
import json

URL = "http://127.0.0.1:4179/morrows/ui/"
KEY = "morrows.oauth_browser_session.v1"
server = {"access":"valid-2", "refresh":"refresh-1", "next":2, "refresh_calls":0, "api_calls":0, "unauth":0}

def handler(route):
    request = route.request
    url = request.url
    if url.endswith("/morrows/auth/oauth/token"):
        params = parse_qs(request.post_data or "")
        grant = params.get("refresh_token",[""])[0]
        if grant != server["refresh"]:
            return route.fulfill(status=400, content_type="application/json", body='{"error":"invalid_grant"}')
        server["refresh_calls"] += 1
        server["next"] += 1
        server["access"] = f'valid-{server["next"]}'
        server["refresh"] = f'refresh-{server["next"]}'
        return route.fulfill(status=200, content_type="application/json", body=json.dumps({"access_token":server["access"],"refresh_token":server["refresh"],"expires_in":3600}))
    if "/morrows/api/" in url:
        server["api_calls"] += 1
        authorization = request.headers.get("authorization","")
        if authorization != "Bearer "+server["access"]:
            server["unauth"] += 1
            return route.fulfill(status=401,content_type="application/json",body='{"error":"expired"}')
        if url.endswith("/operator-session"):
            return route.fulfill(status=200,content_type="application/json",body='{"authenticated":true,"role":"admin"}')
        if "/assignment-requests" in url:
            return route.fulfill(status=200, content_type="application/json",body='{"items":[],"next_offset":null}')
        return route.fulfill(status=200,content_type="application/json",body="[]")
    return route.continue_()

with sync_playwright() as p:
    browser = p.chromium.launch(headless=True)
    context = browser.new_context()
    context.route("**/morrows/ui/assets/**", lambda route: route.continue_(url=route.request.url.replace("/morrows/ui/assets/", "/assets/")))
    context.route("**/morrows/api/**", handler)
    context.route("**/morrows/auth/oauth/token", handler)
    context.add_init_script("localStorage.setItem("+json.dumps(KEY)+", JSON.stringify({access:'expired-1',refresh:'refresh-1',client:'client'}))")
    page = context.new_page()
    page.goto(URL, wait_until="domcontentloaded")
    page.get_by_text("Morrows OAuth 已登录 · admin").wait_for(timeout=12000)
    assert server["refresh_calls"] == 1, server
    assert json.loads(page.evaluate("localStorage.getItem("+json.dumps(KEY)+")"))["refresh"] == server["refresh"]
    print("initial remembered token + refresh: PASS")
    saved = context.storage_state()
    page.close()
    context.close()

    # A new browser context (simulated browser restart) gets only durable
    # localStorage, not previous page's JS heap/sessionStorage.
    context2 = browser.new_context(storage_state=saved)
    context2.route("**/morrows/ui/assets/**", lambda route: route.continue_(url=route.request.url.replace("/morrows/ui/assets/", "/assets/")))
    context2.route("**/morrows/api/**", handler)
    context2.route("**/morrows/auth/oauth/token", handler)
    reopened = context2.new_page()
    reopened.goto(URL, wait_until="domcontentloaded")
    reopened.get_by_text("Morrows OAuth 已登录 · admin").wait_for(timeout=12000)
    assert server["refresh_calls"] == 1, server
    print("browser restart + no new OAuth approval: PASS")

    # Expire the access token for both tabs; both should recover using one
    # refresh due to Web Locks and the atomic localStorage record.
    server["access"] = "server-expired-access"
    tab2 = context2.new_page()
    reopened.reload(wait_until="domcontentloaded")
    tab2.goto(URL, wait_until="domcontentloaded")
    reopened.get_by_text("Morrows OAuth 已登录 · admin").wait_for(timeout=12000)
    tab2.get_by_text("Morrows OAuth 已登录 · admin").wait_for(timeout=12000)
    assert server["refresh_calls"] == 2, server
    print("two tabs, stale access, single refresh rotation: PASS")
    print({k:server[k] for k in ["refresh_calls","api_calls","unauth"]})
    context2.close()
    browser.close()
