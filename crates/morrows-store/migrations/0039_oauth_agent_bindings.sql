-- Bind authenticated OAuth client identities to one canonical AgentInstance.
-- OAuth client IDs remain authentication provenance; task/run ownership lives on
-- the canonical AgentInstance after an explicit operator merge.
CREATE TABLE oauth_agent_bindings (
  client_id TEXT PRIMARY KEY,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  source_agent_instance_id TEXT REFERENCES agent_instances(id),
  bound_by TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX idx_oauth_agent_bindings_agent
  ON oauth_agent_bindings(agent_instance_id);
