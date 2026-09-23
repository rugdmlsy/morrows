ALTER TABLE launch_attempts ADD COLUMN restart_run_id TEXT REFERENCES runs(id);

CREATE TABLE run_lsm_bindings (
  run_id TEXT PRIMARY KEY REFERENCES runs(id),
  logical_session_id TEXT NOT NULL UNIQUE,
  capability_id TEXT,
  restart_deadline_at TEXT,
  session_terminalized_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_run_lsm_restart_deadline ON run_lsm_bindings(restart_deadline_at);

CREATE TABLE run_execution_evidence (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id),
  event_id TEXT REFERENCES events(id),
  kind TEXT NOT NULL,
  lsm_ref TEXT NOT NULL,
  start_seq INTEGER,
  end_seq INTEGER,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_run_execution_evidence_event ON run_execution_evidence(event_id,created_at);
