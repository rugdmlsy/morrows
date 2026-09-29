# morrow-runtime

`morrow-runtime` is the Morrows execution plane. It is a source fork of the existing Local Shell MCP (LSM) runtime/worker implementation, kept inside the Morrows repository so its protocol and lifecycle can evolve around Morrows semantics without changing the standalone LSM project.

Morrows is authoritative for Task, AgentInstance, Machine, Assignment, Session, Memory and execution lifecycle. `morrow-runtime` owns machine-local execution primitives: worker connectivity, process supervision, shell/files/jobs/PTY/workspaces, runtime adapters, logs and low-level evidence.

This initial fork intentionally keeps much of the proven LSM implementation while the Morrows-specific control protocol is extracted. Standalone LSM remains independent and is not modified by this fork.

## Authentication boundary

Public Morrows OAuth is implemented by `morrows-server`, not this runtime.
This package exposes no OAuth metadata, registration, authorization, token endpoint
or Morrows proxy. Production uses internal credentials plus scoped execution
capabilities; public clients use `https://mcp.xycdev.com/morrows`. See
[the control-plane OAuth documentation](../docs/morrows-oauth.md).
