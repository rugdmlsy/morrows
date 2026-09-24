CREATE TABLE agent_credentials (
  id TEXT PRIMARY KEY,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  token_hash TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK(kind IN ('runtime','bridge')),
  label TEXT NOT NULL,
  run_id TEXT REFERENCES runs(id),
  expires_at TEXT NOT NULL,
  revoked_at TEXT,
  created_at TEXT NOT NULL,
  CHECK((kind='runtime' AND run_id IS NOT NULL) OR kind='bridge')
);

CREATE INDEX idx_agent_credentials_agent
  ON agent_credentials(agent_instance_id,kind,created_at DESC);

CREATE INDEX idx_agent_credentials_active_hash
  ON agent_credentials(token_hash,expires_at)
  WHERE revoked_at IS NULL;

CREATE INDEX idx_agent_credentials_run
  ON agent_credentials(run_id)
  WHERE run_id IS NOT NULL;
