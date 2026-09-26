CREATE TABLE task_continuation_policies (
  task_id TEXT PRIMARY KEY REFERENCES tasks(id),
  enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
  agent_ids_json TEXT NOT NULL CHECK(json_valid(agent_ids_json)),
  updated_at TEXT NOT NULL
);
