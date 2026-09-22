ALTER TABLE launch_attempts ADD COLUMN resume_from_attempt_id TEXT REFERENCES launch_attempts(id);

DROP INDEX idx_launch_attempt_active_assignment;
CREATE UNIQUE INDEX idx_launch_attempt_active_assignment
  ON launch_attempts(assignment_id)
  WHERE status IN ('queued','starting','running','awaiting_agent');

CREATE TABLE launch_instructions (
  id TEXT PRIMARY KEY,
  launch_attempt_id TEXT NOT NULL REFERENCES launch_attempts(id),
  actor_type TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  body TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_launch_instructions_attempt
  ON launch_instructions(launch_attempt_id,created_at,id);
