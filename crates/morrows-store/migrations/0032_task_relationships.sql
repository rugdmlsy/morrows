CREATE TABLE task_relationships(
    source_task_id TEXT NOT NULL REFERENCES tasks(id),
    target_task_id TEXT NOT NULL REFERENCES tasks(id),
    relation_type TEXT NOT NULL,
    created_by_actor_id TEXT NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    PRIMARY KEY(source_task_id,target_task_id,relation_type),
    CHECK(source_task_id != target_task_id)
);

CREATE INDEX idx_task_relationships_target
ON task_relationships(target_task_id,relation_type,created_at);

CREATE INDEX idx_task_relationships_source
ON task_relationships(source_task_id,relation_type,created_at);
