CREATE TABLE task_chains (
 id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
 archived INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE task_chain_members (
 chain_id TEXT NOT NULL REFERENCES task_chains(id), task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id),
 join_mode TEXT NOT NULL DEFAULT 'all' CHECK(join_mode IN ('all','any')),
 PRIMARY KEY(chain_id,task_id)
);
CREATE TABLE task_groups (
 id TEXT PRIMARY KEY, name TEXT NOT NULL, description TEXT NOT NULL DEFAULT '',
 archived INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE TABLE task_group_members (
 group_id TEXT NOT NULL REFERENCES task_groups(id), task_id TEXT NOT NULL REFERENCES tasks(id),
 PRIMARY KEY(group_id,task_id)
);

-- Rebuild the legacy dependency table so control-plane chain edits can keep truthful
-- provenance without pretending that a human/operator action came from an AgentInstance.
ALTER TABLE task_dependencies RENAME TO task_dependencies_legacy;
CREATE TABLE task_dependencies (
 task_id TEXT NOT NULL REFERENCES tasks(id),
 depends_on_task_id TEXT NOT NULL REFERENCES tasks(id),
 created_by TEXT NULL REFERENCES agent_instances(id),
 created_by_actor_id TEXT NOT NULL,
 created_at TEXT NOT NULL,
 chain_id TEXT NULL REFERENCES task_chains(id),
 condition_json TEXT NOT NULL DEFAULT '{"kind":"unconditional"}' CHECK(json_valid(condition_json)),
 PRIMARY KEY(task_id,depends_on_task_id),
 CHECK(task_id != depends_on_task_id)
);
INSERT INTO task_dependencies(task_id,depends_on_task_id,created_by,created_by_actor_id,created_at)
SELECT task_id,depends_on_task_id,created_by,'agent:' || created_by,created_at
FROM task_dependencies_legacy;
DROP TABLE task_dependencies_legacy;
CREATE INDEX idx_dependencies_predecessor ON task_dependencies(depends_on_task_id);
CREATE INDEX idx_dependencies_chain ON task_dependencies(chain_id);
