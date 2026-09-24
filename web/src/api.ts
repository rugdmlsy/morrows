export function getOperatorToken() {
  return window.localStorage.getItem("morrows.operatorToken") || "";
}

export function setOperatorToken(token: string) {
  const value = token.trim();
  if (value) window.localStorage.setItem("morrows.operatorToken", value);
  else window.localStorage.removeItem("morrows.operatorToken");
  window.dispatchEvent(new CustomEvent("morrows-operator-token-changed"));
}

export async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const operatorToken = path.startsWith("/api/") ? getOperatorToken() : "";
  const response = await fetch(path, {
    ...init,
    headers: {
      "Content-Type": "application/json",
      ...(operatorToken ? { Authorization: `Bearer ${operatorToken}` } : {}),
      ...(init?.headers || {}),
    },
  });
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `${response.status} ${response.statusText}`);
  }
  return response.json();
}
