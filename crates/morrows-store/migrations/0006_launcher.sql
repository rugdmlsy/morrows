CREATE TABLE launch_profiles (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  adapter TEXT NOT NULL,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  program TEXT NOT NULL,
  codex_home TEXT,
  default_cwd TEXT,
  model TEXT,
  enabled INTEGER NOT NULL DEFAULT 1 CHECK(enabled IN (0,1)),
  metadata_json TEXT NOT NULL DEFAULT '{}' CHECK(json_valid(metadata_json)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_launch_profiles_agent ON launch_profiles(agent_instance_id, enabled, created_at);

CREATE TABLE launch_attempts (
  id TEXT PRIMARY KEY,
  assignment_id TEXT NOT NULL REFERENCES assignments(id),
  task_id TEXT NOT NULL REFERENCES tasks(id),
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  launch_profile_id TEXT NOT NULL REFERENCES launch_profiles(id),
  run_id TEXT REFERENCES runs(id),
  job_id TEXT REFERENCES jobs(id),
  status TEXT NOT NULL,
  cwd TEXT,
  external_session_ref TEXT,
  pid INTEGER,
  exit_code INTEGER,
  stdout_path TEXT,
  stderr_path TEXT,
  error TEXT,
  created_at TEXT NOT NULL,
  started_at TEXT,
  ended_at TEXT
);
CREATE INDEX idx_launch_attempts_task ON launch_attempts(task_id, created_at DESC);
CREATE INDEX idx_launch_attempts_assignment ON launch_attempts(assignment_id, created_at DESC);
CREATE INDEX idx_launch_attempts_status ON launch_attempts(status, created_at);
CREATE UNIQUE INDEX idx_launch_attempt_active_assignment
  ON launch_attempts(assignment_id)
  WHERE status IN ('queued','starting','running');
CREATE UNIQUE INDEX idx_launch_attempt_job ON launch_attempts(job_id) WHERE job_id IS NOT NULL;
