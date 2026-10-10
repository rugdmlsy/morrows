// Regression: remembered Morrows OAuth must survive tab closure and refresh
// token rotation must not revoke a live browser's 90-day grant.
import assert from "node:assert/strict";
import { test } from "node:test";

class MemoryStorage {
  #entries = new Map();
  getItem(k) { return this.#entries.get(k) ?? null; }
  setItem(k, v) { this.#entries.set(k, String(v)); }
  removeItem(k) { this.#entries.delete(k); }
  clear() { this.#entries.clear(); }
}
const sessionStorage = new MemoryStorage();
const localStorage = new MemoryStorage();
const events = [];
globalThis.window = {
  sessionStorage, localStorage,
  location: { pathname: "/morrows/ui/", origin: "https://example.org" },
  addEventListener(type, fn) { events.push([type, fn]); },
  dispatchEvent() {},
};
globalThis.CustomEvent = class { constructor(type) { this.type = type; } };
let lock = Promise.resolve();
Object.defineProperty(globalThis, "navigator", { configurable: true, value: {
  locks: { request(_key, cb) {
    const scheduled = lock.then(cb, cb);
    lock = scheduled.then(() => {}, () => {});
    return scheduled;
  } },
}});
const { api, getMorrowsOAuthToken, setMorrowsOAuthSession, setMorrowsOAuthToken } = await import("../src/api.ts");
const recordKey = "morrows.oauth_browser_session.v1";
const response = (status, result) => ({
  status, ok: status >= 200 && status < 300,
  headers: new Headers({ "content-type": "application/json" }),
  text: async () => JSON.stringify(result),
  json: async () => result,
});

test("saved OAuth survives tab-local storage loss and migrates older tab grants", async () => {
  setMorrowsOAuthSession("access-a", "refresh-a", "client");
  assert.equal(getMorrowsOAuthToken(), "access-a");
  sessionStorage.clear();
  assert.equal(getMorrowsOAuthToken(), "access-a");
  assert.deepEqual(JSON.parse(localStorage.getItem(recordKey)), {access:"access-a",refresh:"refresh-a",client:"client"});

  setMorrowsOAuthToken("");
  assert.equal(localStorage.getItem(recordKey), null);
  sessionStorage.setItem("morrows.oauth_access_token", "old-access");
  sessionStorage.setItem("morrows.oauth_refresh_token", "old-refresh");
  sessionStorage.setItem("morrows.oauth_client_id", "old-client");
  assert.equal(getMorrowsOAuthToken(), "old-access");
  assert.equal(JSON.parse(localStorage.getItem(recordKey)).refresh, "old-refresh");
  assert.equal(sessionStorage.getItem("morrows.oauth_refresh_token"), null);
  setMorrowsOAuthToken("");
});

test("simultaneous expired-access requests rotate the refresh token once", async () => {
  setMorrowsOAuthSession("expired-a", "refresh-a", "client");
  let refreshCalls = 0;
  globalThis.fetch = async (url, init={}) => {
    if (url.endsWith("/oauth/token")) {
      refreshCalls++;
      assert.equal(new URLSearchParams(init.body).get("refresh_token"), "refresh-a");
      await new Promise((resolve) => setTimeout(resolve, 10));
      return response(200, {access_token:"valid-b",refresh_token:"refresh-b"});
    }
    const auth = new Headers(init.headers).get("Authorization");
    return auth === "Bearer valid-b" ? response(200, { authenticated:true, role:"admin" }) : response(401, {error:"expired"});
  };
  const verified = await Promise.all(Array.from({length: 8}, () => api("/api/operator-session")));
  assert.equal(verified.length, 8);
  assert.ok(verified.every((item) => item.authenticated));
  assert.equal(refreshCalls, 1);
  assert.deepEqual(JSON.parse(localStorage.getItem(recordKey)), {access:"valid-b",refresh:"refresh-b",client:"client"});
  setMorrowsOAuthToken("");
});

test("new browser tab with retained refresh grant authenticates before first API request", async () => {
  localStorage.setItem(recordKey, JSON.stringify({access:"",refresh:"remembered",client:"client"}));
  let refreshCalls=0;
  let unauthenticatedRequests=0;
  globalThis.fetch=async (url, init={}) => {
    if (url.endsWith("/oauth/token")) {
      refreshCalls++;
      assert.equal(new URLSearchParams(init.body).get("refresh_token"),"remembered");
      return response(200, {access_token:"renewed-access",refresh_token:"renewed-refresh"});
    }
    const authorization = new Headers(init.headers).get("Authorization");
    if (!authorization) unauthenticatedRequests++;
    return authorization === "Bearer renewed-access" ? response(200,{authenticated:true}) : response(401,{error:"missing"});
  };
  assert.equal((await api("/api/operator-session")).authenticated,true);
  assert.equal(refreshCalls,1);
  assert.equal(unauthenticatedRequests,0);
  setMorrowsOAuthToken("");
});

test("temporary failure does not erase a still renewable grant; logout clears local record", async () => {
  setMorrowsOAuthSession("expired-x","refresh-x","client");
  globalThis.fetch=async (url)=>url.endsWith("/oauth/token")
    ? response(503,{error:"service_unavailable"})
    : response(401,{error:"expired"});
  await assert.rejects(()=>api("/api/operator-session"), /expired/);
  assert.equal(JSON.parse(localStorage.getItem(recordKey)).refresh,"refresh-x");
  setMorrowsOAuthToken("");
  assert.equal(localStorage.getItem(recordKey),null);
});
