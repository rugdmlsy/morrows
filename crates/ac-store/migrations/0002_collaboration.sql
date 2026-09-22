CREATE TABLE artifacts (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 created_by TEXT NOT NULL REFERENCES agent_instances(id),
 title TEXT NOT NULL, uri TEXT NOT NULL, description TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE INDEX idx_artifacts_task ON artifacts(task_id, created_at);
CREATE TABLE decisions (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 created_by TEXT NOT NULL REFERENCES agent_instances(id),
 title TEXT NOT NULL, rationale TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE INDEX idx_decisions_task ON decisions(task_id, created_at);
CREATE TABLE message_threads (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 created_by TEXT NOT NULL REFERENCES agent_instances(id), title TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE INDEX idx_threads_task ON message_threads(task_id, created_at);
CREATE TABLE messages (
 id TEXT PRIMARY KEY, thread_id TEXT NOT NULL REFERENCES message_threads(id),
 created_by TEXT NOT NULL REFERENCES agent_instances(id), body TEXT NOT NULL, created_at TEXT NOT NULL
);
CREATE INDEX idx_messages_thread ON messages(thread_id, created_at);
CREATE TABLE handoffs (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL REFERENCES tasks(id),
 source_run_id TEXT NOT NULL UNIQUE REFERENCES runs(id),
 created_by TEXT NOT NULL REFERENCES agent_instances(id),
 context_revision_id TEXT NOT NULL REFERENCES context_revisions(id),
 content_json TEXT NOT NULL CHECK(json_valid(content_json)), created_at TEXT NOT NULL
);
CREATE INDEX idx_handoffs_task ON handoffs(task_id, created_at);
CREATE TABLE handoff_artifacts (
 handoff_id TEXT NOT NULL REFERENCES handoffs(id), artifact_id TEXT NOT NULL REFERENCES artifacts(id),
 PRIMARY KEY(handoff_id, artifact_id)
);
CREATE TABLE handoff_decisions (
 handoff_id TEXT NOT NULL REFERENCES handoffs(id), decision_id TEXT NOT NULL REFERENCES decisions(id),
 PRIMARY KEY(handoff_id, decision_id)
);
CREATE TABLE task_dependencies (
 task_id TEXT NOT NULL REFERENCES tasks(id), depends_on_task_id TEXT NOT NULL REFERENCES tasks(id),
 created_by TEXT NOT NULL REFERENCES agent_instances(id), created_at TEXT NOT NULL,
 PRIMARY KEY(task_id, depends_on_task_id), CHECK(task_id != depends_on_task_id)
);
