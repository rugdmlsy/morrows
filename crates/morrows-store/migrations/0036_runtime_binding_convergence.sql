-- Profiles created before this cutover had an implicit local default.
-- Freeze that historical meaning explicitly; newly registered profiles default to morrow_runtime.
UPDATE launch_profiles
SET metadata_json=json_set(metadata_json,'$.execution_backend','local')
WHERE adapter IN ('codex_cli','codebuddy_cli')
  AND json_extract(metadata_json,'$.execution_backend') IS NULL;

UPDATE runs SET stop_reason='waiting_for_runtime_job' WHERE stop_reason='waiting_for_lsm_job';

CREATE TABLE run_runtime_bindings (
  run_id TEXT PRIMARY KEY REFERENCES runs(id),
  runtime_scope_id TEXT NOT NULL UNIQUE,
  capability_id TEXT,
  restart_deadline_at TEXT,
  scope_terminalized_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
INSERT INTO run_runtime_bindings(
  run_id,runtime_scope_id,capability_id,restart_deadline_at,scope_terminalized_at,created_at,updated_at
)
SELECT run_id,logical_session_id,capability_id,restart_deadline_at,session_terminalized_at,created_at,updated_at
FROM run_lsm_bindings;
CREATE INDEX idx_run_runtime_restart_deadline
  ON run_runtime_bindings(restart_deadline_at);

CREATE TABLE run_runtime_provisioning (
  run_id TEXT PRIMARY KEY REFERENCES runs(id),
  subject TEXT NOT NULL,
  restart_deadline_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
INSERT INTO run_runtime_provisioning(run_id,subject,restart_deadline_at,created_at,updated_at)
SELECT run_id,subject,restart_deadline_at,created_at,updated_at
FROM run_lsm_provisioning;
CREATE INDEX idx_run_runtime_provisioning_deadline
  ON run_runtime_provisioning(restart_deadline_at);

CREATE TABLE runtime_job_terminal_events (
  event_id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL,
  source_machine TEXT NOT NULL,
  runtime_scope_id TEXT,
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
INSERT INTO runtime_job_terminal_events(
  event_id,job_id,source_machine,runtime_scope_id,attempt,status,exit_code,completed_at,
  terminal_reason,summary_ref,result_json,payload_json,received_at
)
SELECT event_id,job_id,source_machine,logical_session_id,attempt,status,exit_code,completed_at,
       terminal_reason,summary_ref,result_json,payload_json,received_at
FROM lsm_job_terminal_events;
CREATE INDEX idx_runtime_job_events_match
  ON runtime_job_terminal_events(source_machine,job_id,runtime_scope_id,completed_at);

DROP INDEX idx_one_active_run_job_wait;
DROP INDEX idx_run_job_wait_match;
ALTER TABLE run_job_waits RENAME TO run_job_waits_legacy_0036;

CREATE TABLE run_job_waits (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id),
  source_machine TEXT NOT NULL,
  job_id TEXT NOT NULL,
  runtime_scope_id TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('pending','ready','queued','cancelled')),
  resume_mode TEXT CHECK(resume_mode IN ('continue','reconcile')),
  resume_plan TEXT NOT NULL,
  reason TEXT NOT NULL,
  terminal_event_id TEXT REFERENCES runtime_job_terminal_events(event_id),
  terminal_status TEXT,
  terminal_reason TEXT,
  resume_launch_attempt_id TEXT REFERENCES launch_attempts(id),
  registered_at TEXT NOT NULL,
  ready_at TEXT,
  queued_at TEXT
);
INSERT INTO run_job_waits(
  id,run_id,source_machine,job_id,runtime_scope_id,status,resume_mode,resume_plan,reason,
  terminal_event_id,terminal_status,terminal_reason,resume_launch_attempt_id,registered_at,ready_at,queued_at
)
SELECT id,run_id,source_machine,job_id,logical_session_id,status,resume_mode,resume_plan,reason,
       terminal_event_id,terminal_status,terminal_reason,resume_launch_attempt_id,registered_at,ready_at,queued_at
FROM run_job_waits_legacy_0036;
DROP TABLE run_job_waits_legacy_0036;

CREATE UNIQUE INDEX idx_one_active_run_job_wait
  ON run_job_waits(run_id) WHERE status IN ('pending','ready');
CREATE INDEX idx_run_job_wait_match
  ON run_job_waits(source_machine,job_id,runtime_scope_id,status);
