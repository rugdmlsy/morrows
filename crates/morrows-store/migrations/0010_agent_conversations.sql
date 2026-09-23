CREATE TABLE conversations (
  id TEXT PRIMARY KEY,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  title TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open','archived')),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_conversations_agent_updated
  ON conversations(agent_instance_id,status,updated_at DESC,id DESC);

CREATE TABLE conversation_messages (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  author_type TEXT NOT NULL CHECK(author_type IN ('human','agent','system')),
  author_agent_instance_id TEXT REFERENCES agent_instances(id),
  body TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('queued','delivered')),
  created_at TEXT NOT NULL,
  CHECK(
    (author_type='agent' AND author_agent_instance_id IS NOT NULL)
    OR (author_type!='agent' AND author_agent_instance_id IS NULL)
  )
);
CREATE INDEX idx_conversation_messages_history
  ON conversation_messages(conversation_id,created_at DESC,id DESC);
CREATE INDEX idx_conversation_messages_agent_queue
  ON conversation_messages(status,author_type,created_at);
