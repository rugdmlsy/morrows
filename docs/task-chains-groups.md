# Task Chains and Task Groups

## Task Chain

A Task Chain is a named DAG layered on the existing task_dependencies graph. Existing
dependencies remain valid and keep unconditional all semantics even when they do not
belong to a named chain.

Each chain member has a join mode:
- all (default): every incoming chain edge must resolve eligible.
- any: one eligible incoming edge is enough; the node is excluded only after every
  incoming branch is conclusively excluded.

Edges are either unconditional or compare a JSON Pointer in the predecessor's latest
completed executor Run result_json against an exact JSON value. Free text is never
parsed to select branches.

The server derives a Task Gate for every task: eligible, waiting, or excluded. Gate
enforcement is centralized across executor claim, Run start/intake, dispatcher
evaluation, begin-execution, and completion. Reviewer behavior remains compatible with
legacy dependencies and is not blocked by the executor gate.

Cancelling a predecessor excludes its outgoing edge. Rework creates a separate Task and
does not mutate the completed predecessor result. Downstream nodes inherit exclusion
when every route through their predecessor is excluded.

A task belongs to at most one named Task Chain so node-level join mode stays unambiguous.
Task Groups can still collect tasks from any number of different chains.

## Task Group

A Task Group is only a collection. It creates no dependency edges and never merges
Assignment, Run, intake, interview, audit, or memory state.

Group batch assignment evaluates every member independently. Eligible members receive
their own executor Assignment; chain-gated members return blocked; other per-task
errors return failed. No shared Run is created.

## Surfaces

Operator REST exposes chain/group CRUD, membership, edges, gate reads, and group batch
assignment. Employee MCP exposes task_gate, task_chain_get, task_group_get, plus legacy
dependency add/remove tools. The WebUI Task Management page provides Tasks, Task Chains,
and Task Groups views, and normal Task detail displays its current gate.
