ALTER TABLE artifacts ADD COLUMN kind TEXT NOT NULL DEFAULT 'other';
ALTER TABLE messages ADD COLUMN message_type TEXT NOT NULL DEFAULT 'note';
ALTER TABLE messages ADD COLUMN recipient_agent_instance_id TEXT REFERENCES agent_instances(id);
ALTER TABLE messages ADD COLUMN recipient_role TEXT;
ALTER TABLE messages ADD COLUMN reply_to_message_id TEXT REFERENCES messages(id);
ALTER TABLE messages ADD COLUMN correlation_id TEXT;
ALTER TABLE messages ADD COLUMN requires_response INTEGER NOT NULL DEFAULT 0 CHECK(requires_response IN (0,1));
ALTER TABLE messages ADD COLUMN status TEXT NOT NULL DEFAULT 'sent';
ALTER TABLE handoffs ADD COLUMN status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','accepted'));
ALTER TABLE handoffs ADD COLUMN accepted_by_run_id TEXT REFERENCES runs(id)
 CHECK((status='pending' AND accepted_by_run_id IS NULL) OR (status='accepted' AND accepted_by_run_id IS NOT NULL));
