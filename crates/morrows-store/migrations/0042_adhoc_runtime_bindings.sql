CREATE TABLE adhoc_runtime_bindings (
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id) ON DELETE CASCADE,
  machine_id TEXT NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
  runtime_scope_id TEXT UNIQUE,
  generation INTEGER NOT NULL DEFAULT 1 CHECK(generation >= 1),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(agent_instance_id, machine_id)
);

CREATE INDEX idx_adhoc_runtime_bindings_scope
  ON adhoc_runtime_bindings(runtime_scope_id);
