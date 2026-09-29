export function getOperatorToken() {
  return window.sessionStorage.getItem("morrows.operatorToken") || window.localStorage.getItem("morrows.operatorToken") || "";
}

export const MORROWS_OAUTH_TOKEN_KEY = "morrows.oauth_access_token";
export const MORROWS_OAUTH_REFRESH_TOKEN_KEY = "morrows.oauth_refresh_token";
export const MORROWS_OAUTH_CLIENT_ID_KEY = "morrows.oauth_client_id";
const LEGACY_MORROWS_OAUTH_TOKEN_KEY = "morrows.runtime.oauth_access_token";
const LEGACY_LSM_OAUTH_TOKEN_KEY = "lsm.ui.access_token";

export function isHostedUnderMorrows() {
  return window.location.pathname === "/morrows/ui"
    || window.location.pathname.startsWith("/morrows/ui/");
}

export function getMorrowsOAuthToken() {
  const current = window.sessionStorage.getItem(MORROWS_OAUTH_TOKEN_KEY) || "";
  if (current) return current;
  const legacy = window.sessionStorage.getItem(LEGACY_MORROWS_OAUTH_TOKEN_KEY)
    || window.sessionStorage.getItem(LEGACY_LSM_OAUTH_TOKEN_KEY)
    || "";
  if (legacy) window.sessionStorage.setItem(MORROWS_OAUTH_TOKEN_KEY, legacy);
  return legacy;
}

export function setMorrowsOAuthToken(token: string) {
  const value = token.trim();
  if (value) {
    window.sessionStorage.setItem(MORROWS_OAUTH_TOKEN_KEY, value);
  } else {
    window.sessionStorage.removeItem(MORROWS_OAUTH_TOKEN_KEY);
    window.sessionStorage.removeItem(MORROWS_OAUTH_REFRESH_TOKEN_KEY);
    window.sessionStorage.removeItem(MORROWS_OAUTH_CLIENT_ID_KEY);
  }
  window.sessionStorage.removeItem(LEGACY_MORROWS_OAUTH_TOKEN_KEY);
  window.sessionStorage.removeItem(LEGACY_LSM_OAUTH_TOKEN_KEY);
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
  window.sessionStorage.setItem(MORROWS_OAUTH_TOKEN_KEY, access);
  window.sessionStorage.setItem(MORROWS_OAUTH_REFRESH_TOKEN_KEY, refresh);
  window.sessionStorage.setItem(MORROWS_OAUTH_CLIENT_ID_KEY, client);
  window.dispatchEvent(new CustomEvent("morrows-operator-token-changed"));
}

async function refreshMorrowsOAuthToken() {
  const refreshToken = window.sessionStorage.getItem(MORROWS_OAUTH_REFRESH_TOKEN_KEY) || "";
  const clientId = window.sessionStorage.getItem(MORROWS_OAUTH_CLIENT_ID_KEY) || "";
  if (!refreshToken || !clientId) return "";

  try {
    const response = await fetch("/morrows/auth/oauth/token", {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded", Accept: "application/json" },
      body: new URLSearchParams({
        grant_type: "refresh_token",
        client_id: clientId,
        refresh_token: refreshToken,
        resource: `${window.location.origin}/morrows`,
      }),
    });
    const result = await response.json();
    if (!response.ok || !result.access_token || !result.refresh_token) {
      setMorrowsOAuthToken("");
      return "";
    }
    setMorrowsOAuthSession(result.access_token, result.refresh_token, clientId);
    return String(result.access_token);
  } catch {
    return "";
  }
}

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
  const operatorToken = path.startsWith("/api/") && !publicLogin
    ? (isHostedUnderMorrows() ? getMorrowsOAuthToken() : getOperatorToken())
    : "";
  const explicitAuthorization = new Headers(init?.headers).has("Authorization");
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
      && isHostedUnderMorrows()
      && !!operatorToken
      && !explicitAuthorization
    ) {
      const refreshed = await refreshMorrowsOAuthToken();
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
