# morrow-runtime architecture and migration

`morrow-runtime` is Morrows' managed execution plane. It is forked from the standalone Local Shell MCP (LSM) runtime code, but it has a different ownership boundary:

- **Morrows Control Plane** owns Project, Task, Assignment, Run, Session, AgentInstance, Account, Machine, dispatch, permissions, Human Interview, Memory/Milestone/Handoff and provider lifecycle semantics.
- **morrow-runtime** owns worker registration/heartbeat, machine-local process execution, RuntimeScope/Logical Session, Job/Shell/files, provider process supervision and low-level audit/evidence.
- **Provider Plane** owns provider-private model threads and resume references.
- **Standalone LSM remains independent.** It may continue to provide ChatGPT/WebUI OAuth, administration, repair and ARP workflows. Morrows does not use its control key as the provider execution runtime.

## Machine authority

A remote LaunchProfile sets:

```json
{"execution_backend":"morrow_runtime"}
```

The target is not an arbitrary runtime hostname. Morrows resolves:

`AgentInstance.machine_id -> Machine -> runtime worker`

The worker name defaults to `Machine.name`; `Machine.metadata.morrow_runtime_worker` is an explicit compatibility override. `Machine.metadata.morrow_runtime_workdir` may provide the default worker-local enrollment directory.

Remote profiles retain absolute `program` and `default_cwd` syntax, but those paths are validated on the selected worker, not on the Morrows server. `local` remains the compatibility backend.

## Runtime worker enrollment

The runtime control API is loopback-only. Morrows exposes operator APIs that bind enrollment to a canonical Machine:

- `GET /api/runtime-workers`
- `POST /api/machines/{machine_id}/runtime-invite`

The latter creates a short-lived invite whose worker name is derived from that Machine. The generated join script downloads a checksum-pinned worker bundle, selects Python >= 3.11, and can run foreground or install the worker user service.

The worker bundle vendors runtime Python dependencies such as `httpx`; it must not assume that the host Python environment already contains controller dependencies.

## Agent process execution

Both Task LaunchAttempts and direct Morrows Session runtime attempts use the same executor selection.

For `morrow_runtime`:

1. Morrows creates or replays a runtime Logical Session with an idempotency key.
2. Morrows issues a scoped runtime capability when the Agent is authorized to execute.
3. Morrows issues the normal short-lived `mrw_agent_*` employee credential.
4. The target worker materializes private generated files and starts the provider process as a durable runtime Job.
5. Morrows monitors the durable `runtime_id`, handles cancellation/termination, stores provider resume references, and performs runtime cleanup.

Task intake may have transport without an execution capability. Implementation authority is still controlled by the Morrows Assignment phase.

Direct Session runtimes receive a dedicated runtime Logical Session and scoped capability; they remain Morrows Sessions rather than Task Runs.

## Restart and lost-response behavior

`runtime_id` is the Morrows LaunchAttempt or SessionRuntimeAttempt ID. Launch on the worker is idempotent: replaying an existing complete runtime ID returns its current status instead of starting another provider process.

On Morrows restart:

- local child-process launches retain the legacy interrupted/failure reconciliation;
- managed Task runtimes keep the existing Run and restore a monitor for the same runtime ID;
- managed direct Session runtimes are preserved; queued attempts are idempotently replayed and running attempts restore status monitoring;
- transient controller/worker unavailability does not terminalize the Run;
- an explicit reachable-runtime response that the durable runtime does not exist is treated as deterministic runtime loss rather than polled forever.

Provider session references remain adapter state; they are not Morrows Session identities.

## HTTP boundaries

Production keeps the sidecar on loopback, default `127.0.0.1:8790`.

The existing TLS route `/morrows/ui/*` already proxies directly to Morrows and is reused without changing standalone LSM/Caddy:

- `https://mcp.xycdev.com/morrows/ui/agent-mcp` — direct Morrows Agent MCP. It preserves and validates `mrw_agent_*` Bearer credentials.
- `https://mcp.xycdev.com/morrows/ui/runtime/mcp` — scoped morrow-runtime MCP.
- `https://mcp.xycdev.com/morrows/ui/runtime/remote/*` and join/bundle routes — runtime worker transport.

Morrows' runtime proxy allowlist deliberately excludes `/api/control`, OAuth, UI and arbitrary sidecar paths. Runtime control stays loopback and requires `MORROWS_RUNTIME_CONTROL_KEY`.

The normal `https://mcp.xycdev.com/morrows` endpoint remains the standalone-LSM OAuth bridge for ChatGPT/browser clients. Provider Agents must use `MORROWS_AGENT_MCP_URL`, not that OAuth bridge, because the bridge replaces public Authorization with validated client provenance.

## Production deployment

Source-of-truth rules remain unchanged:

1. edit, test, commit and push only from the Mac canonical checkout/worktree;
2. never edit production source on the VPS;
3. deploy only through `scripts/deploy-vps.sh`.

The deploy script:

- creates the pinned `morrow-runtime/.venv` with `uv sync --frozen`;
- installs `morrow-runtime.service`;
- creates `/home/morrow/.config/morrows/runtime.env` with a private control key;
- starts and health-checks the runtime sidecar before Morrows;
- verifies the exact pushed Git commit and immutable deployment-guard artifacts.

Standalone `local-shell-mcp.service` is not replaced or modified by this deployment.

## Migration

The rollout is intentionally non-disruptive:

1. deploy the sidecar while existing LaunchProfiles remain `execution_backend=local`;
2. enroll a test Machine worker through Morrows;
3. bind a test AgentInstance to that Machine;
4. create/select a LaunchProfile with `execution_backend=morrow_runtime`;
5. verify Task and direct Session remote launches, restart recovery and cancellation;
6. migrate additional profiles selectively.

There is no requirement to migrate standalone LSM workers or disable standalone LSM.

## Public OAuth resource binding

Morrows MCP is the protected resource `https://mcp.xycdev.com/morrows`.
Its OAuth issuer and registration/authorization/token endpoints are hosted under
`https://mcp.xycdev.com/morrows/auth`. These URIs serve different purposes: token
`aud` and protected-resource metadata name `/morrows`, while `iss` names
`/morrows/auth`. The 401 challenge links to the RFC 9728 resource metadata URL
`https://mcp.xycdev.com/.well-known/oauth-protected-resource/morrows`.
Caddy sends this exact path to the embedded runtime, preserving standalone
LSM discovery at the origin's bare well-known endpoint. The old metadata URL
under `/morrows/auth` remains a compatible alias. Advertising `/morrows/auth` as
the resource causes strict MCP clients to reject metadata before sending even
previously stored credentials.

The deployment probe checks both fields through the public edge. Existing
standalone-LSM token migration stays restricted to the existing Morrows path
and configured legacy issuer, audience and signing secret; the new resource
binding does not broaden that compatibility rule.
