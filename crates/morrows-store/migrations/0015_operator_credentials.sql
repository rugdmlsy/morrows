CREATE TABLE operator_credentials (
  id TEXT PRIMARY KEY,
  token_hash TEXT NOT NULL UNIQUE,
  label TEXT NOT NULL,
  role TEXT NOT NULL CHECK(role IN ('viewer','operator','admin')),
  expires_at TEXT NOT NULL,
  revoked_at TEXT,
  created_at TEXT NOT NULL
);

CREATE INDEX idx_operator_credentials_active_hash
  ON operator_credentials(token_hash,expires_at)
  WHERE revoked_at IS NULL;

CREATE INDEX idx_operator_credentials_role
  ON operator_credentials(role,created_at DESC);
