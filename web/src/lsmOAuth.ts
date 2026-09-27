import { setLsmOAuthToken } from "./api";

const PENDING_KEY = "morrows.lsm.oauth_pending";
const SCOPES = "shell:read shell:write shell:execute browser:use file:share remote:use";

type PendingOAuth = {
  client_id: string;
  verifier: string;
  state: string;
  redirect_uri: string;
};

function base64Url(bytes: Uint8Array) {
  let raw = "";
  for (const byte of bytes) raw += String.fromCharCode(byte);
  return btoa(raw).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function randomValue() {
  return base64Url(crypto.getRandomValues(new Uint8Array(48)));
}

async function challenge(verifier: string) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return base64Url(new Uint8Array(digest));
}

function redirectUri() {
  return `${window.location.origin}/morrows/ui/`;
}

export async function startLsmOAuth() {
  const uri = redirectUri();
  const registration = await fetch("/oauth/register", {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify({ client_name: "Morrows WebUI", redirect_uris: [uri] }),
  });
  const registered = await registration.json();
  if (!registration.ok || !registered.client_id) {
    throw new Error(registered.error_description || registered.error || `OAuth client registration failed: ${registration.status}`);
  }

  const verifier = randomValue();
  const state = randomValue();
  const pending: PendingOAuth = { client_id: registered.client_id, verifier, state, redirect_uri: uri };
  window.sessionStorage.setItem(PENDING_KEY, JSON.stringify(pending));

  const authorize = new URL("/oauth/authorize", window.location.origin);
  authorize.searchParams.set("response_type", "code");
  authorize.searchParams.set("client_id", pending.client_id);
  authorize.searchParams.set("redirect_uri", pending.redirect_uri);
  authorize.searchParams.set("scope", SCOPES);
  authorize.searchParams.set("resource", window.location.origin);
  authorize.searchParams.set("code_challenge", await challenge(verifier));
  authorize.searchParams.set("code_challenge_method", "S256");
  authorize.searchParams.set("state", state);
  window.location.assign(authorize);
}

export async function completeLsmOAuthCallback() {
  const current = new URL(window.location.href);
  const code = current.searchParams.get("code");
  if (!code) return false;

  const raw = window.sessionStorage.getItem(PENDING_KEY);
  if (!raw) throw new Error("LSM OAuth request state is missing. Start authentication again.");
  const pending = JSON.parse(raw) as PendingOAuth;
  if (current.searchParams.get("state") !== pending.state) {
    throw new Error("LSM OAuth state verification failed.");
  }

  const body = new URLSearchParams({
    grant_type: "authorization_code",
    code,
    client_id: pending.client_id,
    redirect_uri: pending.redirect_uri,
    code_verifier: pending.verifier,
  });
  const response = await fetch("/oauth/token", {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "application/json" },
    body,
  });
  const result = await response.json();
  if (!response.ok || !result.access_token) {
    throw new Error(result.error_description || result.error || "LSM OAuth token exchange failed.");
  }

  setLsmOAuthToken(result.access_token);
  window.sessionStorage.removeItem(PENDING_KEY);
  current.searchParams.delete("code");
  current.searchParams.delete("state");
  current.searchParams.delete("iss");
  window.history.replaceState({}, "", `${current.pathname}${current.search}${current.hash}`);
  return true;
}
