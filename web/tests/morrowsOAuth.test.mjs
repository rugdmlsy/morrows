import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import ts from 'typescript';

// Execute the actual browser module after transpilation. Stub only its token
// storage adapter and browser I/O so the redirect parameters remain observable.
test('WebUI requests the protected resource, distinct from the issuer', async () => {
  const source = await readFile(new URL('../src/morrowsOAuth.ts', import.meta.url), 'utf8');
  const compiled = ts.transpileModule(source.replace(
    'import { setMorrowsOAuthToken } from "./api";',
    'const setMorrowsOAuthToken = () => {};'
  ), { compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2023 } }).outputText;
  let destination;
  const storage = new Map();
  const saved = { window: globalThis.window, fetch: globalThis.fetch };
  globalThis.window = {
    location: { origin: 'https://console.example', assign: url => { destination = url; } },
    sessionStorage: { setItem: (k, v) => storage.set(k, v) },
  };
  globalThis.fetch = async (url, options) => {
    assert.equal(url, '/morrows/auth/oauth/register');
    assert.deepEqual(JSON.parse(options.body).redirect_uris, ['https://console.example/morrows/ui/']);
    return { ok: true, json: async () => ({ client_id: 'test-client' }) };
  };
  try {
    const module = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`);
    await module.startMorrowsOAuth();
    assert.equal(destination.pathname, '/morrows/auth/oauth/authorize');
    assert.equal(destination.searchParams.get('resource'), 'https://console.example/morrows');
    assert.equal(destination.searchParams.get('redirect_uri'), 'https://console.example/morrows/ui/');
    assert.equal(destination.searchParams.get('state'), JSON.parse([...storage.values()][0]).state);
  } finally {
    globalThis.window = saved.window;
    globalThis.fetch = saved.fetch;
  }
});
