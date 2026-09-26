export function getOperatorToken() {
  return window.sessionStorage.getItem("morrows.operatorToken") || window.localStorage.getItem("morrows.operatorToken") || "";
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
  const hostedUnderMorrows = window.location.pathname === "/morrows/ui"
    || window.location.pathname.startsWith("/morrows/ui/");
  if (hostedUnderMorrows && path.startsWith("/api/")) return `/morrows${path}`;
  return path;
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const publicLogin = path === "/api/operator-login" || path === "/api/operator-login/status";
  const operatorToken = path.startsWith("/api/") && !publicLogin ? getOperatorToken() : "";
  const response = await fetch(publicPath(path), {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...(operatorToken ? { Authorization: `Bearer ${operatorToken}` } : {}),
      ...(init?.headers || {}),
    },
  });
  if (!response.ok) {
    const text = await response.text();
    let message = text || `${response.status} ${response.statusText}`;
    try { message = JSON.parse(text).error || message; } catch { /* Plain-text proxy errors remain readable. */ }
    if (response.status === 401 && !new Headers(init?.headers).has("Authorization")) {
      window.dispatchEvent(new CustomEvent("morrows-operator-auth-required"));
    }
    throw new ApiError(response.status, message);
  }
  return response.json();
}
