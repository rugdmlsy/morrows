# Morrows public OAuth

Every public MCP client uses `https://mcp.xycdev.com/morrows`. The issuer is
`https://mcp.xycdev.com/morrows/auth`. `morrows-server` implements discovery,
dynamic registration, authorization-code S256 PKCE, access tokens, refresh-token
rotation and revocation. The shared Caddy edge routes these directly to port 8787.
Neither standalone LSM nor morrow-runtime participates in public authentication.

## Credentials and persistence

- Registration always creates a distinct client; names and redirects never deduplicate
  identities. Each Codex profile keeps its own client registration and credentials.
- An authorization screen displays the client, redirect and requested scope and
  reuses Morrows operator authentication. Normal `morrows` consent accepts operator
  or admin authority; `morrows:control` requires admin. If the browser has no usable
  operator session, the consent page starts the existing SSH-approved operator-login
  flow and resumes automatically after approval. There is no separate OAuth approval
  secret. Browser approval retains a single-use CSRF prompt and a scoped
  Secure/HttpOnly/SameSite operator cookie.
- Authorization codes expire after five minutes and are single use. Redirect URI,
  client, resource and S256 verifier must match. Only HTTPS or loopback HTTP
  redirect URIs are accepted, without fragments or embedded credentials.
- Opaque access tokens last one hour. Refresh tokens rotate on every successful
  exchange. Grants expire absolutely after 90 days; rotation never extends this.
- Refresh reuse revokes the entire grant, including its access tokens and successor
  refresh tokens. Clients must serialize refreshes; a lost refresh response requires
  reauthorization if retrying the spent token. There is deliberately no replay grace.
- `POST /morrows/auth/oauth/revoke` accepts `client_id` and `token` as form fields.
  Revoking an access or refresh token revokes its entire grant. Unknown tokens are
  successful no-ops. Other clients cannot revoke the grant.
- SQLite migration 0038 stores the registry in `morrows_oauth_state`; only secret
  hashes are persisted. A write lock covers reading and updating state, including
  rejection outcomes that revoke a replayed grant. Back up the Morrows database;
  runtime state directories are unrelated. Registry operations serialize in SQLite;
  this design targets the current small fleet rather than a high-volume public IdP.
- `morrows` is the default MCP scope. `morrows:control` explicitly grants WebUI
  administration and is displayed during approval. Public identity always comes
  from server-side verification, never caller-provided provenance headers.

Discovery URLs:

- `/.well-known/oauth-protected-resource/morrows`
- `/.well-known/oauth-authorization-server/morrows/auth`
- `/morrows/auth/.well-known/oauth-authorization-server` (issuer-path alias)

## Bounded cutover

The managed production deploy performs a one-time compatibility migration without
making LSM or morrow-runtime a continuing authentication dependency:

- it removes the retired `MORROWS_OAUTH_ADMIN_PIN`; OAuth approval is delegated to
  the existing Morrows operator login/approval model;
- it copies the existing Cloudflare tunnel credential into Morrows private
  `service.env`, after which the Morrows connector no longer reads the LSM env;
- it copies the previous morrow-runtime and standalone-LSM OAuth signing secrets
  into explicitly named `MORROWS_OAUTH_LEGACY_*` verifier variables and records a
  fixed 30-day `MORROWS_OAUTH_LEGACY_UNTIL`; later deploys never extend it;
- after migration 0038 is live, it imports legacy dynamic client registrations into
  `morrows_oauth_state` so those client IDs can reauthorize without reading runtime
  state again.

Legacy bearer verification is read-only compatibility: Morrows never issues another
legacy JWT. New authorization always produces Morrows-owned opaque access/refresh
tokens. When the fixed window expires, old JWTs stop authenticating even if the old
signing material remains in the private env until the operator removes it. Historical
AgentInstances remain for task history and no existing task/assignment identity is
rewritten by the schema migration.

`MORROWS_AGENT_MCP_URL` is removed. The old `/morrows/ui/agent-mcp` and UI MCP
aliases return 410 at the edge. Public launchers use the profile's OAuth session;
they do not pass `mrw_agent_*` as an MCP bearer. Run-bound credentials remain in
internal CLI/employee operations. Launching under a new OS/profile requires that
profile to complete OAuth beforehand; unattended launch does not approve itself.

## Production steps (parent agent / operator)

1. Review and merge the Mac commit, then push and deploy using the managed deploy
   script. Do not edit production source. This task does not perform deployment.
2. Before deployment, back up the Morrows database. The managed deploy removes any
   retired OAuth approval PIN and performs the bounded legacy-token / client-registration
   migration described above; it does not print authentication secrets.
3. Remove obsolete Agent-MCP and runtime public OAuth settings after verification.
   Runtime uses `LOCAL_SHELL_MCP_AUTH_MODE=internal`; its private signing secret
   defaults to its own persisted runtime state. Re-enroll any old private container
   clients whose prior runtime signing secret is intentionally no longer used.
4. Install the new Caddy routes and service units through deployment. Confirm
   metadata, an unauthenticated MCP 401 challenge, authorization, refresh and revoke.
5. Reinstall/update `~/.codex`, `~/.codex-personal`, `~/.codex-mentor2` to `/morrows`,
   removing old bearer/header overrides. Each Codex profile performs its own OAuth
   login and stores its own client/access/refresh credentials. Consent reuses an
   already-approved Morrows operator session or displays the existing SSH approval
   command; normal MCP clients do not require admin approval. Existing ChatGPT and
   WebUI legacy bearer sessions remain usable during the fixed migration window and
   can reauthorize onto the new issuer without depending on runtime state.
6. Verify MCP and refresh while runtime is stopped/restarted, then while standalone
   LSM is stopped/restarted. Restart Morrows and verify the same client identity and
   refresh token still work. Verify runtime capability execution separately.
7. A rollback to the old server cannot interpret these new opaque tokens. Restore
   a coordinated code/edge/database backup if rollback is required, and reauthorize
   clients; do not mix old edge routes and new server authentication.

The WebUI keeps its access token in tab session storage; its control scope is
separate from the MCP default. Internal `mrw_operator_*` RBAC remains available for
operator CLI administration. Approval secrets and bearer tokens must not be logged.
