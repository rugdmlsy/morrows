ALTER TABLE assignments
ADD COLUMN phase TEXT NOT NULL DEFAULT 'implementing'
CHECK (phase IN ('context_review','human_interview','ready','implementing'));

CREATE TABLE assignment_intakes (
  assignment_id TEXT PRIMARY KEY REFERENCES assignments(id) ON DELETE CASCADE,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  project_id TEXT NULL REFERENCES projects(id),
  project_memory_head TEXT NULL,
  project_memory_next_offset INTEGER NULL,
  project_memory_complete INTEGER NOT NULL DEFAULT 0 CHECK (project_memory_complete IN (0,1)),
  project_memory_read_at TEXT NULL,
  context_package_id TEXT NULL REFERENCES context_packages(id),
  context_revision_id TEXT NULL REFERENCES context_revisions(id),
  context_package_read_at TEXT NULL,
  understanding TEXT NOT NULL DEFAULT '',
  constraints_json TEXT NOT NULL DEFAULT '{}',
  plan_json TEXT NOT NULL DEFAULT '[]',
  questions_json TEXT NOT NULL DEFAULT '[]',
  unresolved_questions_json TEXT NOT NULL DEFAULT '[]',
  interview_status TEXT NOT NULL DEFAULT 'not_started'
    CHECK (interview_status IN ('not_started','pending','revision_requested','approved')),
  human_response TEXT NULL,
  approved_by_actor_id TEXT NULL,
  approved_at TEXT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_assignment_intakes_task_agent
ON assignment_intakes(task_id,agent_instance_id,updated_at DESC);
