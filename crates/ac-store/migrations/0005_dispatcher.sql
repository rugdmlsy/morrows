CREATE TABLE task_dispatch_policies (
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  required_capabilities_json TEXT NOT NULL DEFAULT '[]' CHECK(json_valid(required_capabilities_json)),
  profile_id TEXT REFERENCES agent_profiles(id),
  account_id TEXT REFERENCES accounts(id),
  machine_id TEXT REFERENCES machines(id),
  heartbeat_ttl_seconds INTEGER NOT NULL CHECK(heartbeat_ttl_seconds > 0),
  capacity_ttl_seconds INTEGER NOT NULL CHECK(capacity_ttl_seconds > 0),
  lease_seconds INTEGER NOT NULL CHECK(lease_seconds >= 30),
  enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY(task_id, role)
);
CREATE INDEX idx_dispatch_policy_enabled ON task_dispatch_policies(enabled, role, task_id);

CREATE TABLE dispatch_decisions (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  outcome TEXT NOT NULL,
  selected_agent_instance_id TEXT REFERENCES agent_instances(id),
  assignment_id TEXT REFERENCES assignments(id),
  preview_json TEXT NOT NULL CHECK(json_valid(preview_json)),
  created_at TEXT NOT NULL
);
CREATE INDEX idx_dispatch_decisions_task ON dispatch_decisions(task_id, created_at DESC, id DESC);
CREATE TRIGGER dispatch_decision_no_update BEFORE UPDATE ON dispatch_decisions BEGIN
 SELECT RAISE(ABORT,'dispatch decisions are append-only');
END;
CREATE TRIGGER dispatch_decision_no_delete BEFORE DELETE ON dispatch_decisions BEGIN
 SELECT RAISE(ABORT,'dispatch decisions are append-only');
END;
