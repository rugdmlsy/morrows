-- Existing operator-created groups remain operator-managed (NULL owner).
-- Agent-created groups can only be mutated by the creating AgentInstance.
ALTER TABLE task_groups ADD COLUMN owner_agent_instance_id TEXT REFERENCES agent_instances(id);
