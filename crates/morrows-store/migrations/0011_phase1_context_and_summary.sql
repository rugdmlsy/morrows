-- Phase 1: Context snapshot pinning for runs
CREATE TABLE IF NOT EXISTS run_context_revisions (
  run_id TEXT PRIMARY KEY REFERENCES runs(id) ON DELETE CASCADE,
  context_revision_id TEXT NOT NULL REFERENCES context_revisions(id),
  pinned_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_run_context_revisions_ctx ON run_context_revisions(context_revision_id);

-- Phase 1: Structured Conversation Summary Revisions
CREATE TABLE IF NOT EXISTS conversation_summary_revisions (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  previous_revision_id TEXT NULL REFERENCES conversation_summary_revisions(id),
  covers_until_message_id TEXT NULL REFERENCES conversation_messages(id),
  goal TEXT NOT NULL DEFAULT '',
  current_state TEXT NOT NULL DEFAULT '',
  important_findings_json TEXT NOT NULL DEFAULT '[]',
  decisions_json TEXT NOT NULL DEFAULT '[]',
  blockers_json TEXT NOT NULL DEFAULT '[]',
  unresolved_questions_json TEXT NOT NULL DEFAULT '[]',
  next_steps_json TEXT NOT NULL DEFAULT '[]',
  deterministic_facts_json TEXT NOT NULL DEFAULT '{}',
  created_by TEXT NOT NULL DEFAULT 'system',
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_conv_summary_rev_conv_id ON conversation_summary_revisions(conversation_id, created_at DESC);

-- Phase 1: Context Packages for cross-agent work transfer
CREATE TABLE IF NOT EXISTS context_packages (
  id TEXT PRIMARY KEY,
  work_item_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  objective TEXT NOT NULL,
  summary_json TEXT NULL,
  context_snapshot_id TEXT NULL REFERENCES context_revisions(id),
  memory_refs_json TEXT NOT NULL DEFAULT '[]',
  decision_refs_json TEXT NOT NULL DEFAULT '[]',
  artifact_refs_json TEXT NOT NULL DEFAULT '[]',
  changed_files_json TEXT NOT NULL DEFAULT '[]',
  verified_results_json TEXT NOT NULL DEFAULT '[]',
  blockers_json TEXT NOT NULL DEFAULT '[]',
  unresolved_questions_json TEXT NOT NULL DEFAULT '[]',
  next_action TEXT NOT NULL DEFAULT '',
  source_run_id TEXT NULL REFERENCES runs(id),
  source_agent_id TEXT NULL REFERENCES agent_instances(id),
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_context_packages_work_item ON context_packages(work_item_id, created_at DESC);
