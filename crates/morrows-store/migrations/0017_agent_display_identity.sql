ALTER TABLE accounts ADD COLUMN email TEXT;
ALTER TABLE agent_instances ADD COLUMN display_name TEXT;
ALTER TABLE agent_instances ADD COLUMN archived_at TEXT;

UPDATE agent_instances SET display_name = name WHERE display_name IS NULL;

CREATE UNIQUE INDEX idx_accounts_provider_email
  ON accounts(provider, email)
  WHERE email IS NOT NULL AND email <> '';

CREATE UNIQUE INDEX idx_agent_instances_active_display_name
  ON agent_instances(display_name)
  WHERE archived_at IS NULL AND display_name IS NOT NULL;
