ALTER TABLE conversations RENAME TO sessions;
ALTER TABLE conversation_messages RENAME TO session_messages;
ALTER TABLE session_messages RENAME COLUMN conversation_id TO session_id;
ALTER TABLE conversation_summary_revisions RENAME TO session_summary_revisions;
ALTER TABLE session_summary_revisions RENAME COLUMN conversation_id TO session_id;

DROP INDEX IF EXISTS idx_conversations_agent_updated;
DROP INDEX IF EXISTS idx_conversation_messages_history;
DROP INDEX IF EXISTS idx_conversation_messages_agent_queue;
DROP INDEX IF EXISTS idx_conv_summary_rev_conv_id;

CREATE INDEX idx_sessions_agent_updated
  ON sessions(agent_instance_id,status,updated_at DESC,id DESC);
CREATE INDEX idx_session_messages_history
  ON session_messages(session_id,created_at DESC,id DESC);
CREATE INDEX idx_session_messages_agent_queue
  ON session_messages(status,author_type,created_at);
CREATE INDEX idx_session_summary_rev_session_id
  ON session_summary_revisions(session_id,created_at DESC);

DROP INDEX IF EXISTS idx_agent_deliveries_agent_queue;
DROP INDEX IF EXISTS idx_agent_deliveries_task_queue;
DROP INDEX IF EXISTS idx_agent_deliveries_claim;

CREATE TABLE agent_deliveries_session_migration (
  id TEXT PRIMARY KEY,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  task_id TEXT REFERENCES tasks(id),
  kind TEXT NOT NULL CHECK(kind IN ('session_message','launch_instruction')),
  source_id TEXT NOT NULL,
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  status TEXT NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','claimed','delivered')),
  claimed_by_launch_attempt_id TEXT REFERENCES launch_attempts(id),
  claimed_at TEXT,
  delivered_by TEXT,
  delivered_at TEXT,
  created_at TEXT NOT NULL,
  UNIQUE(kind,source_id)
);

INSERT INTO agent_deliveries_session_migration(
  id,agent_instance_id,task_id,kind,source_id,payload_json,status,
  claimed_by_launch_attempt_id,claimed_at,delivered_by,delivered_at,created_at
)
SELECT
  id,
  agent_instance_id,
  task_id,
  CASE kind WHEN 'conversation_message' THEN 'session_message' ELSE kind END,
  source_id,
  CASE kind
    WHEN 'conversation_message' THEN json_remove(
      json_set(payload_json, '$.session_id', json_extract(payload_json, '$.conversation_id')),
      '$.conversation_id'
    )
    ELSE payload_json
  END,
  status,
  claimed_by_launch_attempt_id,
  claimed_at,
  delivered_by,
  delivered_at,
  created_at
FROM agent_deliveries;

DROP TABLE agent_deliveries;
ALTER TABLE agent_deliveries_session_migration RENAME TO agent_deliveries;

CREATE INDEX idx_agent_deliveries_agent_queue
  ON agent_deliveries(agent_instance_id,status,created_at,id);
CREATE INDEX idx_agent_deliveries_task_queue
  ON agent_deliveries(agent_instance_id,task_id,status,created_at,id);
CREATE INDEX idx_agent_deliveries_claim
  ON agent_deliveries(claimed_by_launch_attempt_id,status);

UPDATE events SET entity_type='session' WHERE entity_type='conversation';
UPDATE events
SET event_type='session.' || substr(event_type, length('conversation.') + 1)
WHERE event_type LIKE 'conversation.%';
