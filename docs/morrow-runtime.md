# morrow-runtime architecture and migration

`morrow-runtime` is Morrows' managed execution plane. It is forked from the standalone Local Shell MCP (LSM) runtime code, but it has a different ownership boundary:

- **Morrows Control Plane** owns Project, Task, Assignment, Run, Session, AgentInstance, Account, Machine, dispatch, permissions, Human Interview, Memory/Milestone/Handoff and provider lifecycle semantics.
- **morrow-runtime** owns worker registration/heartbeat, machine-local process execution, internal RuntimeScope, Job/Shell/files, provider process supervision and low-level audit/evidence. Its historical `Logical Session` object is an implementation detail, not a second Morrows work/session lifecycle.
- **Provider Plane** owns provider-private model threads and resume references.
- **Standalone LSM remains independent and rescue-only for ordinary Morrows work.** It may continue to authenticate its own clients and serve diagnostics, emergency repair and ARP workflows, but Morrows authentication never passes through it and Morrows does not use its control key as the provider execution runtime.

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

1. Morrows creates or replays one internal RuntimeScope with an idempotency key derived from the Run. The legacy `/sessions` API is an implementation detail; Morrows records only `RunRuntimeBinding.runtime_scope_id`.
2. Morrows issues a scoped runtime capability when the Agent is authorized to execute.
3. Morrows issues the normal short-lived `mrw_agent_*` employee credential.
4. The target worker materializes private generated files and starts the provider process as a durable runtime Job.
5. Morrows monitors the durable `runtime_id`, handles cancellation/termination, stores provider resume references, and performs runtime cleanup.

Task intake may have transport without an execution capability. Implementation authority is still controlled by the Morrows Assignment phase.

Direct Morrows Session runtimes receive a dedicated internal RuntimeScope and scoped capability; the conversation remains a Morrows Session while RuntimeScope is execution plumbing.

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

All public Morrows MCP clients use `https://mcp.xycdev.com/morrows` with
Morrows-owned OAuth. Caddy sends MCP, OAuth and metadata directly to
`morrows-server` on port 8787. There is no Agent-token MCP endpoint.

- `/morrows/ui/runtime/mcp` carries only scoped runtime execution capabilities.
- `/morrows/ui/runtime/remote/*` and join/bundle routes carry runtime worker transport.
- Runtime control stays loopback-only and requires `MORROWS_RUNTIME_CONTROL_KEY`.

The runtime proxy excludes OAuth, `/api/control`, UI and arbitrary sidecar paths.
Runtime has no OAuth server or public Morrows bridge. Private invite-bound runtime
credentials use `LOCAL_SHELL_MCP_AUTH_MODE=internal` and a runtime-only signing
secret. They are not accepted by Morrows MCP. `mrw_agent_*` remains available to
internal Run/CLI operations but is not injected into provider MCP configuration.

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

## Default execution policy

For `codex_cli` and `codebuddy_cli` LaunchProfiles, omitted `metadata.execution_backend` means `morrow_runtime`. Migration 0036 copies legacy `run_lsm_bindings` / `run_lsm_provisioning` into canonical runtime tables and converts job-wait correlation from `logical_session_id` to `runtime_scope_id`; migration 0037 moves Morrows-managed legacy CLI profiles to `morrow_runtime`. `local` remains an explicit compatibility/test backend; standalone LSM is not a normal execution backend.

The control-plane identity chain is `Task -> Assignment -> Run -> RunRuntimeBinding -> RuntimeScope -> worker/process/jobs`. Only Task/Assignment/Run own context, milestones, handoffs and completion. RuntimeScope is low-level execution state. A runtime restart or lost scope-binding response replays onto the same Run rather than creating a second work execution. `RuntimeJobTerminalEvent` and `/internal/runtime/job-events` are canonical; `/internal/lsm/job-events` remains a standalone-LSM compatibility adapter.

## Public OAuth ownership

The protected resource is `https://mcp.xycdev.com/morrows`; its issuer is `https://mcp.xycdev.com/morrows/auth`. Both are owned by `morrows-server`, not `morrow-runtime` or standalone LSM. OAuth clients, authorization codes, hashed access tokens and refresh-token grant state persist in the Morrows database (migration 0038), independently of runtime restart or removal. RFC 9728 resource metadata is published at `https://mcp.xycdev.com/.well-known/oauth-protected-resource/morrows`. See [morrows-oauth.md](morrows-oauth.md) for refresh rotation and the bounded legacy-token cutover.
