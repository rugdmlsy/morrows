ALTER TABLE memory_entries ADD COLUMN provenance_json TEXT CHECK(provenance_json IS NULL OR json_valid(provenance_json));

CREATE TABLE memory_publications (
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  idempotency_key TEXT NOT NULL,
  input_json TEXT NOT NULL CHECK(json_valid(input_json)),
  memory_id TEXT NOT NULL REFERENCES memory_entries(id),
  PRIMARY KEY(agent_instance_id, idempotency_key)
);

CREATE TABLE assignment_requests (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  role TEXT NOT NULL,
  reason TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','approved','rejected','withdrawn')),
  assignment_id TEXT REFERENCES assignments(id),
  resolution TEXT,
  created_at TEXT NOT NULL,
  resolved_at TEXT
);
CREATE UNIQUE INDEX one_pending_assignment_request
  ON assignment_requests(task_id, agent_instance_id, role) WHERE status='pending';
CREATE INDEX assignment_request_queue ON assignment_requests(status,created_at,id);
