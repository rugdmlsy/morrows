CREATE TABLE memory_entries (
  id TEXT PRIMARY KEY,
  scope_type TEXT NOT NULL CHECK(scope_type IN ('organization','project','agent','task')),
  project_id TEXT REFERENCES projects(id) ON DELETE CASCADE,
  agent_instance_id TEXT REFERENCES agent_instances(id) ON DELETE CASCADE,
  task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
  title TEXT NOT NULL,
  content_json TEXT NOT NULL CHECK(json_valid(content_json)),
  source_kind TEXT NOT NULL,
  source_ref TEXT,
  visibility TEXT NOT NULL DEFAULT 'shared' CHECK(visibility IN ('shared','private')),
  supersedes_memory_id TEXT REFERENCES memory_entries(id),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  CHECK(
    (scope_type='organization' AND project_id IS NULL AND agent_instance_id IS NULL AND task_id IS NULL)
    OR (scope_type='project' AND project_id IS NOT NULL AND agent_instance_id IS NULL AND task_id IS NULL)
    OR (scope_type='agent' AND project_id IS NULL AND agent_instance_id IS NOT NULL AND task_id IS NULL)
    OR (scope_type='task' AND project_id IS NULL AND agent_instance_id IS NULL AND task_id IS NOT NULL)
  )
);

CREATE INDEX idx_memory_entries_scope
  ON memory_entries(scope_type, project_id, agent_instance_id, task_id, created_at DESC);

CREATE INDEX idx_memory_entries_supersedes
  ON memory_entries(supersedes_memory_id)
  WHERE supersedes_memory_id IS NOT NULL;
