ALTER TABLE session_messages ADD COLUMN client_message_id TEXT;
ALTER TABLE session_messages ADD COLUMN recalled_at TEXT;

CREATE UNIQUE INDEX idx_session_messages_client_id
  ON session_messages(session_id, client_message_id)
  WHERE client_message_id IS NOT NULL;
