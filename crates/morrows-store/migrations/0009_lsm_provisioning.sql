CREATE TABLE run_lsm_provisioning (
  run_id TEXT PRIMARY KEY REFERENCES runs(id),
  subject TEXT NOT NULL,
  restart_deadline_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_run_lsm_provisioning_deadline ON run_lsm_provisioning(restart_deadline_at);
