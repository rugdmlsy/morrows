CREATE TABLE agent_deliveries (
  id TEXT PRIMARY KEY,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  task_id TEXT REFERENCES tasks(id),
  kind TEXT NOT NULL CHECK(kind IN ('conversation_message','launch_instruction')),
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

CREATE INDEX idx_agent_deliveries_agent_queue
  ON agent_deliveries(agent_instance_id,status,created_at,id);

CREATE INDEX idx_agent_deliveries_task_queue
  ON agent_deliveries(agent_instance_id,task_id,status,created_at,id);

CREATE INDEX idx_agent_deliveries_claim
  ON agent_deliveries(claimed_by_launch_attempt_id,status);

-- Upgrade compatibility: queued human messages predate the delivery outbox but still
-- represent work that must reach the addressed AgentInstance.
INSERT INTO agent_deliveries(
  id,agent_instance_id,task_id,kind,source_id,payload_json,status,created_at
)
SELECT
  lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-' ||
        hex(randomblob(2)) || '-' || hex(randomblob(2)) || '-' || hex(randomblob(6))),
  c.agent_instance_id,
  NULL,
  'conversation_message',
  m.id,
  json_object(
    'conversation_id', m.conversation_id,
    'message_id', m.id,
    'body', m.body
  ),
  'queued',
  m.created_at
FROM conversation_messages m
JOIN conversations c ON c.id=m.conversation_id
WHERE m.author_type='human' AND m.status='queued';

-- Instructions attached to an active launch may also have been created immediately
-- before this migration. Finished-attempt instructions are history, not new deliveries.
INSERT INTO agent_deliveries(
  id,agent_instance_id,task_id,kind,source_id,payload_json,status,created_at
)
SELECT
  lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-' ||
        hex(randomblob(2)) || '-' || hex(randomblob(2)) || '-' || hex(randomblob(6))),
  a.agent_instance_id,
  a.task_id,
  'launch_instruction',
  i.id,
  json_object(
    'launch_attempt_id', i.launch_attempt_id,
    'instruction_id', i.id,
    'body', i.body
  ),
  'queued',
  i.created_at
FROM launch_instructions i
JOIN launch_attempts a ON a.id=i.launch_attempt_id
WHERE a.status IN ('queued','starting','running','awaiting_agent');
