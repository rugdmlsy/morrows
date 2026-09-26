import { useEffect, useState } from "react";
import { api, ApiError, getOperatorToken, setOperatorToken } from "./api";
import type { Locale } from "./i18n";

type Identity = { authenticated: boolean; role: string; label?: string; expires_at?: string };
type LoginRequest = { id: string; code: string; token: string; expires_at: string };

export default function OperatorLogin({ locale, onLogin }: { locale: Locale; onLogin: () => void }) {
  const zh = locale === "zh-CN";
  const [identity, setIdentity] = useState<Identity | null>(null);
  const [required, setRequired] = useState(false);
  const [open, setOpen] = useState(false);
  const [request, setRequest] = useState<LoginRequest | null>(null);
  const [token, setToken] = useState("");
  const [remember, setRemember] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  useEffect(() => {
    const verify = () => {
      void api<Identity>("/api/operator-session").then(value => {
        setIdentity(value); setRequired(false); setError("");
      }).catch((e: unknown) => {
        setIdentity(null);
        if (e instanceof ApiError && e.status === 401) setRequired(true);
        else setError(e instanceof Error ? e.message : String(e));
      });
    };
    const unauthorized = () => { setIdentity(null); setRequired(true); };
    verify();
    window.addEventListener("morrows-operator-token-changed", verify);
    window.addEventListener("morrows-operator-auth-required", unauthorized);
    return () => {
      window.removeEventListener("morrows-operator-token-changed", verify);
      window.removeEventListener("morrows-operator-auth-required", unauthorized);
    };
  }, []);

  useEffect(() => {
    if (!request) return;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout>;
    // The secret stays in this component until approval, never in a URL, shell
    // command or console. Poll serially so slow responses cannot pile up.
    const poll = async () => {
      if (Date.now() >= Date.parse(request.expires_at)) {
        setError(zh ? "登录请求已过期，请重新发起。" : "Login request expired. Start again.");
        setRequest(null); return;
      }
      try {
        const result = await api<{ status: string }>("/api/operator-login/status", {
          method: "POST", body: JSON.stringify({ id: request.id, token: request.token }),
        });
        if (stopped) return;
        if (result.status === "approved") {
          setOperatorToken(request.token, remember);
          setRequest(null); setOpen(false); setError(""); onLogin(); return;
        }
        if (result.status === "expired") {
          setError(zh ? "登录请求已过期或已撤销，请重新发起。" : "Login expired or revoked. Start again.");
          setRequest(null); return;
        }
      } catch (e) { if (!stopped) setError(e instanceof Error ? e.message : String(e)); }
      if (!stopped) timer = setTimeout(poll, 2000);
    };
    void poll();
    return () => { stopped = true; clearTimeout(timer); };
  }, [request, remember, zh, onLogin]);

  const startLogin = async () => {
    setBusy(true); setError("");
    try {
      setRequest(await api<LoginRequest>("/api/operator-login", {
        method: "POST", body: JSON.stringify({ label: `Browser ${new Date().toISOString()}` }),
      }));
    } catch (e) { setError(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };
  const signInWithToken = async () => {
    const value = token.trim().replace(/^Bearer\s+/i, "");
    if (!value.startsWith("mrw_operator_")) {
      setError(zh ? "需要 Operator 凭据；Agent 凭据不能登录控制台。" : "Use an Operator credential, not an Agent credential."); return;
    }
    setBusy(true); setError("");
    try {
      // Check before replacing a working credential. Failed input never destroys it.
      const verified = await api<Identity>("/api/operator-session", { headers: { Authorization: `Bearer ${value}` } });
      setOperatorToken(value, remember); setIdentity(verified); setToken(""); setOpen(false); onLogin();
    } catch (e) { setError(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  };

  return <>
    <button type="button" className={`auth-button ${identity ? "auth-configured" : ""}`} onClick={() => setOpen(true)}>
      <span className="auth-dot" />
      {identity ? `${zh ? "已登录" : "Signed in"} · ${identity.role}` : required ? (zh ? "登录控制台" : "Sign in") : (zh ? "检查登录" : "Checking login")}
    </button>
    {required && !open && <button type="button" className="operator-signin-hint" onClick={() => setOpen(true)}>
      {zh ? "控制台需要登录；没有旧 token 也可通过 SSH 批准登录。" : "Sign in with an existing token or approve a browser login over SSH."}
    </button>}
    {open && <div className="operator-overlay">
      <section role="dialog" aria-modal="true" aria-labelledby="operator-login-title" className="operator-dialog">
        <h2 id="operator-login-title">{zh ? "控制台登录" : "Console sign in"}</h2>
        {identity && <p>{identity.label} · {identity.role}{identity.expires_at && ` · ${zh ? "到期" : "expires"} ${new Date(identity.expires_at).toLocaleString()}`}</p>}
        <p>{zh ? "没有旧 token 时，在此发起请求，然后用已有服务器 SSH 权限批准。页面上的登录码必须与终端命令一致。" : "Without an old token, request a login and approve its code from your trusted server SSH shell."}</p>
        <label><input type="checkbox" checked={remember} onChange={e => setRemember(e.target.checked)} /> {zh ? "在此浏览器记住登录（否则仅此标签页）" : "Remember in this browser (otherwise this tab only)"}</label>
        {request ? <div className="operator-pairing">
          <strong>{zh ? "等待 SSH 批准" : "Waiting for SSH approval"}: {request.code}</strong>
          <p>{zh ? "在 Morrows 服务目录执行；如使用自定义数据库，请替换路径：" : "Run in the Morrows service directory; replace the database path if customized:"}</p>
          <pre>{`morrows operator-approve --database data/morrows.db --code ${request.code}`}</pre>
          <p>{zh ? "默认授予 operator 权限，有效期 24 小时；登录码 10 分钟后过期。批准后页面自动登录。" : "Default: operator role for 24 hours. The code expires after 10 minutes. This page signs in automatically after approval."}</p>
        </div> : <button type="button" disabled={busy} onClick={() => void startLogin()}>{zh ? "通过 SSH 批准登录" : "Request SSH-approved login"}</button>}
        <details><summary>{zh ? "使用已有 Operator token" : "Use an existing Operator token"}</summary>
          <input type="password" aria-label="Operator token" autoComplete="off" value={token} onChange={e => setToken(e.target.value)} />
          <button type="button" disabled={busy || !token.trim()} onClick={() => void signInWithToken()}>{zh ? "验证并登录" : "Verify and sign in"}</button>
        </details>
        {error && <p role="alert">{error}</p>}
        <div className="operator-dialog-actions">
          {!!getOperatorToken() && <button type="button" onClick={() => { setOperatorToken(""); setIdentity(null); setRequired(true); onLogin(); }}>{zh ? "退出此浏览器" : "Sign out here"}</button>}
          <button type="button" onClick={() => { setOpen(false); setRequest(null); setToken(""); }}>{zh ? "关闭" : "Close"}</button>
        </div>
      </section>
    </div>}
  </>;
}
