CREATE TABLE lsm_job_terminal_events (
  event_id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL,
  source_machine TEXT NOT NULL,
  logical_session_id TEXT,
  attempt INTEGER NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('succeeded','failed','exited','stopped','lost')),
  exit_code INTEGER,
  completed_at TEXT NOT NULL,
  terminal_reason TEXT NOT NULL,
  summary_ref TEXT,
  result_json TEXT,
  payload_json TEXT NOT NULL,
  received_at TEXT NOT NULL
);
CREATE INDEX idx_lsm_job_events_match
  ON lsm_job_terminal_events(source_machine,job_id,logical_session_id,completed_at);

CREATE TABLE run_job_waits (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id),
  source_machine TEXT NOT NULL,
  job_id TEXT NOT NULL,
  logical_session_id TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','ready','queued','cancelled')),
  resume_mode TEXT CHECK(resume_mode IN ('continue','reconcile')),
  resume_plan TEXT NOT NULL,
  reason TEXT NOT NULL,
  terminal_event_id TEXT REFERENCES lsm_job_terminal_events(event_id),
  terminal_status TEXT,
  terminal_reason TEXT,
  resume_launch_attempt_id TEXT REFERENCES launch_attempts(id),
  registered_at TEXT NOT NULL,
  ready_at TEXT,
  queued_at TEXT
);
CREATE UNIQUE INDEX idx_one_active_run_job_wait
  ON run_job_waits(run_id) WHERE status IN ('pending','ready');
CREATE INDEX idx_run_job_wait_match
  ON run_job_waits(source_machine,job_id,logical_session_id,status);
