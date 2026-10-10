
ALTER TABLE tasks ADD COLUMN acceptance_criteria_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE tasks ADD COLUMN acceptance_version INTEGER NOT NULL DEFAULT 1;
CREATE TABLE verification_receipts (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
 executor_run_id TEXT NOT NULL REFERENCES runs(id),
 reviewer_run_id TEXT REFERENCES runs(id),
 criterion_id TEXT NOT NULL,
 acceptance_version INTEGER NOT NULL,
 context_revision_id TEXT,
 kind TEXT NOT NULL,
 verdict TEXT NOT NULL,
 detail_json TEXT NOT NULL,
 created_at TEXT NOT NULL
);
CREATE INDEX verification_receipts_lookup ON verification_receipts(task_id,criterion_id,created_at);
CREATE TABLE completion_records (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
 run_id TEXT NOT NULL REFERENCES runs(id),
 record_json TEXT NOT NULL,
 created_at TEXT NOT NULL
);
