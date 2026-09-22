PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS projects (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'active',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
  id TEXT PRIMARY KEY,
  project_id TEXT NULL REFERENCES projects(id),
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  owner_actor_id TEXT NOT NULL,
  state TEXT NOT NULL,
  priority INTEGER NOT NULL DEFAULT 0,
  current_context_revision_id TEXT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tasks_state_priority ON tasks(state, priority DESC, created_at);

CREATE TABLE IF NOT EXISTS agent_instances (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL DEFAULT 'online',
  capabilities_json TEXT NOT NULL DEFAULT '[]',
  last_heartbeat_at TEXT NOT NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS assignments (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  role TEXT NOT NULL,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  status TEXT NOT NULL,
  acquired_at TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  renewed_at TEXT NOT NULL,
  released_at TEXT NULL,
  release_reason TEXT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_one_active_assignment_per_role
  ON assignments(task_id, role) WHERE status = 'active';
CREATE INDEX IF NOT EXISTS idx_assignment_agent_status ON assignments(agent_instance_id, status);

CREATE TABLE IF NOT EXISTS runs (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  assignment_id TEXT NOT NULL REFERENCES assignments(id),
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  external_session_ref TEXT NULL,
  status TEXT NOT NULL,
  stop_reason TEXT NULL,
  checkpoint_json TEXT NULL,
  result_json TEXT NULL,
  usage_json TEXT NULL,
  started_at TEXT NOT NULL,
  ended_at TEXT NULL,
  metadata_json TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS idx_runs_task_started ON runs(task_id, started_at DESC);

CREATE TABLE IF NOT EXISTS context_revisions (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  version INTEGER NOT NULL,
  parent_revision_id TEXT NULL REFERENCES context_revisions(id),
  goal TEXT NOT NULL DEFAULT '',
  background TEXT NOT NULL DEFAULT '',
  constraints_json TEXT NOT NULL DEFAULT '{}',
  current_summary TEXT NOT NULL DEFAULT '',
  created_by_actor_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(task_id, version)
);
CREATE INDEX IF NOT EXISTS idx_context_task_version ON context_revisions(task_id, version DESC);

CREATE TABLE IF NOT EXISTS events (
  id TEXT PRIMARY KEY,
  actor_type TEXT NOT NULL,
  actor_id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  correlation_id TEXT NULL,
  payload_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_events_entity ON events(entity_type, entity_id, created_at);
CREATE INDEX IF NOT EXISTS idx_events_type ON events(event_type, created_at);

CREATE TABLE IF NOT EXISTS jobs (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'pending',
  available_at TEXT NOT NULL,
  claimed_at TEXT NULL,
  attempt_count INTEGER NOT NULL DEFAULT 0,
  last_error TEXT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_jobs_ready ON jobs(status, available_at);

CREATE TABLE IF NOT EXISTS outbox (
  id TEXT PRIMARY KEY,
  topic TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  created_at TEXT NOT NULL,
  delivered_at TEXT NULL
);
CREATE INDEX IF NOT EXISTS idx_outbox_pending ON outbox(status, created_at);
