export function getOperatorToken() {
  return window.sessionStorage.getItem("morrows.operatorToken") || window.localStorage.getItem("morrows.operatorToken") || "";
}

export const MORROWS_OAUTH_TOKEN_KEY = "morrows.runtime.oauth_access_token";
const LEGACY_LSM_OAUTH_TOKEN_KEY = "lsm.ui.access_token";

export function isHostedUnderMorrows() {
  return window.location.pathname === "/morrows/ui"
    || window.location.pathname.startsWith("/morrows/ui/");
}

export function getMorrowsOAuthToken() {
  return window.sessionStorage.getItem(MORROWS_OAUTH_TOKEN_KEY)
    || window.sessionStorage.getItem(LEGACY_LSM_OAUTH_TOKEN_KEY)
    || "";
}

export function setMorrowsOAuthToken(token: string) {
  const value = token.trim();
  window.sessionStorage.removeItem(LEGACY_LSM_OAUTH_TOKEN_KEY);
  if (value) window.sessionStorage.setItem(MORROWS_OAUTH_TOKEN_KEY, value);
  else window.sessionStorage.removeItem(MORROWS_OAUTH_TOKEN_KEY);
  window.dispatchEvent(new CustomEvent("morrows-operator-token-changed"));
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
  let response: Response;
  try {
    response = await fetch(publicPath(path), {
      ...init,
      headers: {
        "Content-Type": "application/json",
        ...(operatorToken ? { Authorization: `Bearer ${operatorToken}` } : {}),
        ...(init?.headers || {}),
      },
    });
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
