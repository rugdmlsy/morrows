-- A browser keeps the secret; SSH approval promotes only its digest to a credential.
CREATE TABLE operator_login_requests (
  id TEXT PRIMARY KEY,
  code TEXT NOT NULL UNIQUE,
  token_hash TEXT NOT NULL UNIQUE,
  label TEXT NOT NULL,
  created_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  credential_id TEXT REFERENCES operator_credentials(id)
);
