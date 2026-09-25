CREATE TABLE session_runtime_attempts (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  account_id TEXT REFERENCES accounts(id),
  launch_profile_id TEXT NOT NULL REFERENCES launch_profiles(id),
  adapter TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('queued','running','completed','failed')),
  cwd TEXT,
  provider_session_ref TEXT,
  pid INTEGER,
  exit_code INTEGER,
  stdout_path TEXT,
  stderr_path TEXT,
  error TEXT,
  created_at TEXT NOT NULL,
  started_at TEXT,
  ended_at TEXT
);

CREATE UNIQUE INDEX idx_session_runtime_active
  ON session_runtime_attempts(session_id)
  WHERE status IN ('queued','running');

CREATE INDEX idx_session_runtime_history
  ON session_runtime_attempts(session_id,created_at DESC,id DESC);

ALTER TABLE agent_deliveries ADD COLUMN claimed_by_session_runtime_id TEXT REFERENCES session_runtime_attempts(id);
CREATE INDEX idx_agent_deliveries_session_runtime_claim
  ON agent_deliveries(claimed_by_session_runtime_id,status);

ALTER TABLE agent_credentials RENAME TO agent_credentials_legacy;

CREATE TABLE agent_credentials (
  id TEXT PRIMARY KEY,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  token_hash TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK(kind IN ('runtime','bridge','session_runtime')),
  label TEXT NOT NULL,
  run_id TEXT REFERENCES runs(id),
  session_id TEXT REFERENCES sessions(id),
  expires_at TEXT NOT NULL,
  revoked_at TEXT,
  created_at TEXT NOT NULL,
  CHECK(
    (kind='runtime' AND run_id IS NOT NULL AND session_id IS NULL)
    OR (kind='session_runtime' AND run_id IS NULL AND session_id IS NOT NULL)
    OR (kind='bridge' AND run_id IS NULL AND session_id IS NULL)
  )
);

INSERT INTO agent_credentials(
  id,agent_instance_id,token_hash,kind,label,run_id,session_id,
  expires_at,revoked_at,created_at
)
SELECT id,agent_instance_id,token_hash,kind,label,run_id,NULL,
       expires_at,revoked_at,created_at
FROM agent_credentials_legacy;

DROP TABLE agent_credentials_legacy;

CREATE INDEX idx_agent_credentials_agent
  ON agent_credentials(agent_instance_id,kind,created_at DESC);
CREATE INDEX idx_agent_credentials_active_hash
  ON agent_credentials(token_hash,expires_at)
  WHERE revoked_at IS NULL;
CREATE INDEX idx_agent_credentials_run
  ON agent_credentials(run_id)
  WHERE run_id IS NOT NULL;
CREATE INDEX idx_agent_credentials_session
  ON agent_credentials(session_id)
  WHERE session_id IS NOT NULL;
