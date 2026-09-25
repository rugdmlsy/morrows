ALTER TABLE sessions ADD COLUMN project_id TEXT REFERENCES projects(id) ON DELETE SET NULL;
ALTER TABLE sessions ADD COLUMN task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL;

CREATE INDEX idx_sessions_project_updated
  ON sessions(project_id,status,updated_at DESC,id DESC)
  WHERE project_id IS NOT NULL;
CREATE INDEX idx_sessions_task_updated
  ON sessions(task_id,status,updated_at DESC,id DESC)
  WHERE task_id IS NOT NULL;

ALTER TABLE launch_attempts ADD COLUMN session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL;
CREATE INDEX idx_launch_attempts_session
  ON launch_attempts(session_id,created_at DESC)
  WHERE session_id IS NOT NULL;
