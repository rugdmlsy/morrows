ALTER TABLE tasks ADD COLUMN spec_version INTEGER NOT NULL DEFAULT 1;
CREATE TABLE task_revisions (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
 version INTEGER NOT NULL,
 base_version INTEGER NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('applied','pending_ack','rejected')),
 author_actor_id TEXT NOT NULL,
 reason TEXT NOT NULL,
 before_json TEXT NOT NULL,
 after_json TEXT NOT NULL,
 context_revision_before TEXT,
 context_revision_after TEXT,
 ack_run_id TEXT,
 ack_agent_id TEXT,
 ack_impact TEXT,
 ack_plan_json TEXT,
 created_at TEXT NOT NULL,
 resolved_at TEXT,
 UNIQUE(task_id, version)
);
CREATE UNIQUE INDEX idx_task_one_pending_revision
 ON task_revisions(task_id) WHERE status='pending_ack';
CREATE INDEX idx_task_revision_history
 ON task_revisions(task_id,version DESC);
