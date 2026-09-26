CREATE TABLE run_milestones (
  id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL REFERENCES runs(id),
  task_id TEXT NOT NULL REFERENCES tasks(id),
  created_by TEXT NOT NULL REFERENCES agent_instances(id),
  sequence INTEGER NOT NULL CHECK(sequence > 0),
  context_revision_id TEXT REFERENCES context_revisions(id),
  content_json TEXT NOT NULL CHECK(json_valid(content_json)),
  created_at TEXT NOT NULL,
  UNIQUE(run_id, sequence)
);

CREATE INDEX idx_run_milestones_run
  ON run_milestones(run_id, sequence DESC);
CREATE INDEX idx_run_milestones_task
  ON run_milestones(task_id, created_at DESC);

CREATE TABLE run_milestone_artifacts (
  milestone_id TEXT NOT NULL REFERENCES run_milestones(id) ON DELETE CASCADE,
  artifact_id TEXT NOT NULL REFERENCES artifacts(id),
  PRIMARY KEY(milestone_id, artifact_id)
);

CREATE TABLE run_milestone_decisions (
  milestone_id TEXT NOT NULL REFERENCES run_milestones(id) ON DELETE CASCADE,
  decision_id TEXT NOT NULL REFERENCES decisions(id),
  PRIMARY KEY(milestone_id, decision_id)
);

ALTER TABLE handoffs ADD COLUMN milestone_id TEXT REFERENCES run_milestones(id);
CREATE INDEX idx_handoffs_milestone ON handoffs(milestone_id);
