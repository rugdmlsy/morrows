CREATE TABLE agent_identity_reports (
    id TEXT PRIMARY KEY,
    agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id) ON DELETE CASCADE,
    agent_name TEXT,
    account_email TEXT,
    platform TEXT,
    device TEXT,
    reported_at TEXT NOT NULL
);

CREATE INDEX idx_agent_identity_reports_latest
    ON agent_identity_reports(agent_instance_id, reported_at DESC, id DESC);
