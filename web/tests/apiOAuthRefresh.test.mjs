import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

test('WebUI rotates the Morrows refresh token and retries one 401', async () => {
  const source = await readFile(new URL('../src/api.ts', import.meta.url), 'utf8');
  const compiled = ts.transpileModule(source, {
    compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2023 },
  }).outputText;

  const storage = new Map([
    ['morrows.oauth_access_token', 'old-access'],
    ['morrows.oauth_refresh_token', 'old-refresh'],
    ['morrows.oauth_client_id', 'client-a'],
  ]);
  const saved = {
    window: globalThis.window,
    fetch: globalThis.fetch,
    CustomEvent: globalThis.CustomEvent,
  };
  const calls = [];
  globalThis.CustomEvent = class {
    constructor(type) { this.type = type; }
  };
  globalThis.window = {
    location: {
      origin: 'https://console.example',
      pathname: '/morrows/ui/',
    },
    sessionStorage: {
      getItem: key => storage.get(key) || null,
      setItem: (key, value) => storage.set(key, value),
      removeItem: key => storage.delete(key),
    },
    localStorage: {
      getItem: () => null,
      removeItem: () => {},
    },
    dispatchEvent: () => true,
  };
  globalThis.fetch = async (url, options = {}) => {
    calls.push({ url, options });
    if (calls.length === 1) {
      assert.equal(url, '/morrows/api/tasks');
      assert.equal(options.headers.Authorization, 'Bearer old-access');
      return new Response(JSON.stringify({ error: 'expired' }), {
        status: 401,
        headers: { 'content-type': 'application/json' },
      });
    }
    if (calls.length === 2) {
      assert.equal(url, '/morrows/auth/oauth/token');
      const fields = new URLSearchParams(options.body);
      assert.equal(fields.get('grant_type'), 'refresh_token');
      assert.equal(fields.get('client_id'), 'client-a');
      assert.equal(fields.get('refresh_token'), 'old-refresh');
      assert.equal(fields.get('resource'), 'https://console.example/morrows');
      return new Response(JSON.stringify({
        access_token: 'new-access',
        refresh_token: 'new-refresh',
        token_type: 'Bearer',
      }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    }
    assert.equal(url, '/morrows/api/tasks');
    assert.equal(options.headers.Authorization, 'Bearer new-access');
    return new Response(JSON.stringify({ ok: true }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  };

  try {
    const module = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`);
    const result = await module.api('/api/tasks');
    assert.deepEqual(result, { ok: true });
    assert.equal(storage.get('morrows.oauth_access_token'), 'new-access');
    assert.equal(storage.get('morrows.oauth_refresh_token'), 'new-refresh');
    assert.equal(storage.get('morrows.oauth_client_id'), 'client-a');
    assert.equal(calls.length, 3);
  } finally {
    globalThis.window = saved.window;
    globalThis.fetch = saved.fetch;
    globalThis.CustomEvent = saved.CustomEvent;
  }
});
