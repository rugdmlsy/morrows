export function getOperatorToken() {
  return window.sessionStorage.getItem("morrows.operatorToken") || window.localStorage.getItem("morrows.operatorToken") || "";
}

export const MORROWS_OAUTH_TOKEN_KEY = "morrows.oauth_access_token";
export const MORROWS_OAUTH_REFRESH_TOKEN_KEY = "morrows.oauth_refresh_token";
export const MORROWS_OAUTH_CLIENT_ID_KEY = "morrows.oauth_client_id";
// One atomic persistent record: a refresh-token rotation must never save a
// new access token with the old refresh token. Scoped to this browser origin.
const OAUTH_SESSION_KEY = "morrows.oauth_browser_session.v1";
const OAUTH_REFRESH_LOCK = "morrows.oauth_browser_refresh.v1";
const LEGACY_MORROWS_OAUTH_TOKEN_KEY = "morrows.runtime.oauth_access_token";
const LEGACY_LSM_OAUTH_TOKEN_KEY = "lsm.ui.access_token";

type OAuthSession = { access: string; refresh: string; client: string };
let refreshInFlight: Promise<string> | null = null;

export function isHostedUnderMorrows() {
  return window.location.pathname === "/morrows/ui"
    || window.location.pathname.startsWith("/morrows/ui/");
}

function savedOAuthSession(): OAuthSession | null {
  const raw = window.localStorage.getItem(OAUTH_SESSION_KEY);
  if (raw) {
    try {
      const saved = JSON.parse(raw) as OAuthSession;
      if (typeof saved.access === "string" && saved.refresh && saved.client) return saved;
    } catch { /* Invalid browser state must not authorize a session. */ }
    window.localStorage.removeItem(OAUTH_SESSION_KEY);
  }
  // Migrate existing per-tab OAuth credentials on upgrade. Previously they
  // disappeared when the tab closed even though server refresh grants persist.
  const access = window.sessionStorage.getItem(MORROWS_OAUTH_TOKEN_KEY) || "";
  const refresh = window.sessionStorage.getItem(MORROWS_OAUTH_REFRESH_TOKEN_KEY) || "";
  const client = window.sessionStorage.getItem(MORROWS_OAUTH_CLIENT_ID_KEY) || "";
  if (!refresh || !client) return null;
  const saved = { access, refresh, client };
  window.localStorage.setItem(OAUTH_SESSION_KEY, JSON.stringify(saved));
  for (const key of [MORROWS_OAUTH_TOKEN_KEY, MORROWS_OAUTH_REFRESH_TOKEN_KEY, MORROWS_OAUTH_CLIENT_ID_KEY]) {
    window.sessionStorage.removeItem(key);
  }
  return saved;
}

function clearLegacyOAuthKeys() {
  for (const key of [
    MORROWS_OAUTH_TOKEN_KEY, MORROWS_OAUTH_REFRESH_TOKEN_KEY, MORROWS_OAUTH_CLIENT_ID_KEY,
    LEGACY_MORROWS_OAUTH_TOKEN_KEY, LEGACY_LSM_OAUTH_TOKEN_KEY,
  ]) {
    window.sessionStorage.removeItem(key);
  }
}

export function getMorrowsOAuthToken() {
  const session = savedOAuthSession();
  if (session) return session.access;
  return window.sessionStorage.getItem(LEGACY_MORROWS_OAUTH_TOKEN_KEY)
    || window.sessionStorage.getItem(LEGACY_LSM_OAUTH_TOKEN_KEY)
    || "";
}

export function setMorrowsOAuthToken(token: string) {
  const value = token.trim();
  if (value) {
    // Retain the refresh grant when updating an existing browser session.
    const session = savedOAuthSession();
    if (session) window.localStorage.setItem(OAUTH_SESSION_KEY, JSON.stringify({ ...session, access: value }));
    else window.sessionStorage.setItem(MORROWS_OAUTH_TOKEN_KEY, value);
  } else {
    window.localStorage.removeItem(OAUTH_SESSION_KEY);
  }
  clearLegacyOAuthKeys();
  window.dispatchEvent(new CustomEvent("morrows-operator-token-changed"));
}

export function setMorrowsOAuthSession(accessToken: string, refreshToken: string, clientId: string) {
  const access = accessToken.trim();
  const refresh = refreshToken.trim();
  const client = clientId.trim();
  if (!access || !refresh || !client) {
    setMorrowsOAuthToken("");
    return;
  }
  // Persist only Morrows OAuth tokens, not the temporary SSH approval code.
  window.localStorage.setItem(OAUTH_SESSION_KEY, JSON.stringify({ access, refresh, client }));
  clearLegacyOAuthKeys();
  window.dispatchEvent(new CustomEvent("morrows-operator-token-changed"));
}

async function refreshOAuthUnderLock(rejectedAccess: string): Promise<string> {
  const existing = savedOAuthSession();
  if (!existing) return "";
  // Another request/tab already rotated the refresh grant while we waited.
  if (rejectedAccess !== existing.access && existing.access) return existing.access;
  try {
    const response = await fetch("/morrows/auth/oauth/token", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "application/json" },
      body: new URLSearchParams({
        grant_type: "refresh_token",
        client_id: existing.client,
        refresh_token: existing.refresh,
        resource: `${window.location.origin}/morrows`,
      }),
    });
    if (!response.ok) {
      // An invalid/revoked grant is terminal, but transient 5xx/429/proxy
      // failures must never discard a valid remembered browser session.
      if (response.status === 400 || response.status === 401) {
        const latest = savedOAuthSession();
        if (latest?.refresh === existing.refresh) setMorrowsOAuthToken("");
      }
      return "";
    }
    const result = await response.json();
    if (!result.access_token || !result.refresh_token) return "";
    // Do not overwrite an explicit logout (or a new OAuth login) while pending.
    const latest = savedOAuthSession();
    if (latest?.refresh !== existing.refresh) return latest?.access || "";
    setMorrowsOAuthSession(result.access_token, result.refresh_token, existing.client);
    return String(result.access_token);
  } catch {
    return "";
  }
}

function refreshMorrowsOAuthToken(rejectedAccess: string): Promise<string> {
  if (refreshInFlight) return refreshInFlight;
  const refresh = async () => {
    // The authorization server rotates refresh tokens and revokes the entire
    // family on replay. Serialize refresh across tabs, not just within a tab.
    if (navigator.locks?.request) {
      return navigator.locks.request(OAUTH_REFRESH_LOCK, () => refreshOAuthUnderLock(rejectedAccess));
    }
    return refreshOAuthUnderLock(rejectedAccess);
  };
  const ongoing = refresh().finally(() => { refreshInFlight = null; });
  refreshInFlight = ongoing;
  return ongoing;
}

// In a second tab, a login/logout or token rotation also changes the shared
// browser session. The next API call always reads this latest persisted state.
window.addEventListener("storage", (event) => {
  if (event.key === OAUTH_SESSION_KEY) {
    window.dispatchEvent(new CustomEvent("morrows-operator-token-changed"));
  }
});

export function setOperatorToken(token: string, remember = false) {
  const value = token.trim();
  window.localStorage.removeItem("morrows.operatorToken");
  window.sessionStorage.removeItem("morrows.operatorToken");
  if (value) (remember ? window.localStorage : window.sessionStorage).setItem("morrows.operatorToken", value);
  window.dispatchEvent(new CustomEvent("morrows-operator-token-changed"));
}

export class ApiError extends Error {
  status: number;
  constructor(status: number, message: string) { super(message); this.status = status; }
}

function publicPath(path: string) {
  const hostedUnderMorrows = isHostedUnderMorrows();
  if (hostedUnderMorrows && path.startsWith("/api/")) return `/morrows${path}`;
  return path;
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const publicLogin = path === "/api/operator-login" || path === "/api/operator-login/status";
  const hosted = isHostedUnderMorrows();
  const isAuthenticatedApi = path.startsWith("/api/") && !publicLogin;
  const explicitAuthorization = new Headers(init?.headers).has("Authorization");
  let operatorToken = isAuthenticatedApi
    ? (hosted ? getMorrowsOAuthToken() : getOperatorToken())
    : "";
  if (hosted && isAuthenticatedApi && !operatorToken && !explicitAuthorization) {
    // A closed tab may have no access token but still retain its refresh grant.
    operatorToken = await refreshMorrowsOAuthToken("");
  }
  const request = (token: string) => fetch(publicPath(path), {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
      ...(init?.headers || {}),
    },
  });

  let response: Response;
  try {
    response = await request(operatorToken);
    if (
      response.status === 401
      && hosted && isAuthenticatedApi
      && !explicitAuthorization
    ) {
      const refreshed = await refreshMorrowsOAuthToken(operatorToken);
      if (refreshed) response = await request(refreshed);
    }
  } catch {
    throw new ApiError(0, "Service temporarily unavailable; reconnecting automatically.");
  }
  if (!response.ok) {
    const text = await response.text();
    const contentType = response.headers.get("content-type") || "";
    let message = text || `${response.status} ${response.statusText}`;
    if (contentType.includes("text/html") || /^\s*<!doctype html/i.test(text)) {
      message = `Service temporarily unavailable (${response.status} ${response.statusText}); reconnecting automatically.`;
    } else {
      try {
        const parsed = JSON.parse(text);
        message = parsed.error || parsed.detail || message;
      } catch { /* Plain-text proxy errors remain readable. */ }
    }
    if (response.status === 401 && !new Headers(init?.headers).has("Authorization")) {
      window.dispatchEvent(new CustomEvent("morrows-operator-auth-required"));
    }
    throw new ApiError(response.status, message);
  }
  return response.json();
}
