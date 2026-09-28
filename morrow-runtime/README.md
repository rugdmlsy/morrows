# morrow-runtime

`morrow-runtime` is the Morrows execution plane. It is a source fork of the existing Local Shell MCP (LSM) runtime/worker implementation, kept inside the Morrows repository so its protocol and lifecycle can evolve around Morrows semantics without changing the standalone LSM project.

Morrows is authoritative for Task, AgentInstance, Machine, Assignment, Session, Memory and execution lifecycle. `morrow-runtime` owns machine-local execution primitives: worker connectivity, process supervision, shell/files/jobs/PTY/workspaces, runtime adapters, logs and low-level evidence.

This initial fork intentionally keeps much of the proven LSM implementation while the Morrows-specific control protocol is extracted. Standalone LSM remains independent and is not modified by this fork.
