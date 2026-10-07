-- Retire Morrows-owned Sessions from the active Task/Run model.
-- Legacy Session tables remain intact for historical read/migration purposes.

ALTER TABLE message_threads
ADD COLUMN kind TEXT NOT NULL DEFAULT 'collaboration'
CHECK(kind IN ('collaboration','human_agent'));

ALTER TABLE message_threads
ADD COLUMN target_agent_instance_id TEXT REFERENCES agent_instances(id);

CREATE UNIQUE INDEX idx_threads_task_human_agent
ON message_threads(task_id,target_agent_instance_id)
WHERE kind='human_agent' AND target_agent_instance_id IS NOT NULL;

-- A task-bound legacy Session becomes a task-scoped human/agent thread.
-- Preserve the UUID so existing interview/message references can migrate losslessly.
INSERT OR IGNORE INTO message_threads(
  id,task_id,created_by,title,created_at,kind,target_agent_instance_id
)
SELECT
  s.id,s.task_id,s.agent_instance_id,s.title,s.created_at,'human_agent',s.agent_instance_id
FROM sessions s
WHERE s.task_id IS NOT NULL;

-- Collaboration messages need to represent human/system authors as well as Agents.
ALTER TABLE messages RENAME TO messages_pre_session_dedup;

CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  thread_id TEXT NOT NULL REFERENCES message_threads(id) ON DELETE CASCADE,
  created_by TEXT REFERENCES agent_instances(id),
  author_type TEXT NOT NULL DEFAULT 'agent'
    CHECK(author_type IN ('human','agent','system')),
  body TEXT NOT NULL,
  created_at TEXT NOT NULL,
  message_type TEXT NOT NULL DEFAULT 'note',
  recipient_agent_instance_id TEXT REFERENCES agent_instances(id),
  recipient_role TEXT,
  reply_to_message_id TEXT REFERENCES messages(id),
  correlation_id TEXT,
  requires_response INTEGER NOT NULL DEFAULT 0 CHECK(requires_response IN (0,1)),
  status TEXT NOT NULL DEFAULT 'sent',
  client_message_id TEXT,
  recalled_at TEXT,
  CHECK(
    (author_type='agent' AND created_by IS NOT NULL)
    OR (author_type!='agent' AND created_by IS NULL)
  )
);

INSERT INTO messages(
  id,thread_id,created_by,author_type,body,created_at,
  message_type,recipient_agent_instance_id,recipient_role,reply_to_message_id,
  correlation_id,requires_response,status,client_message_id,recalled_at
)
SELECT
  id,thread_id,created_by,'agent',body,created_at,
  message_type,recipient_agent_instance_id,recipient_role,reply_to_message_id,
  correlation_id,requires_response,status,NULL,NULL
FROM messages_pre_session_dedup;

INSERT OR IGNORE INTO messages(
  id,thread_id,created_by,author_type,body,created_at,
  message_type,recipient_agent_instance_id,recipient_role,reply_to_message_id,
  correlation_id,requires_response,status,client_message_id,recalled_at
)
SELECT
  m.id,
  m.session_id,
  m.author_agent_instance_id,
  m.author_type,
  m.body,
  m.created_at,
  'human_agent',
  CASE WHEN m.author_type='human' THEN s.agent_instance_id ELSE NULL END,
  NULL,
  NULL,
  NULL,
  CASE WHEN m.author_type='human' AND m.status='queued' THEN 1 ELSE 0 END,
  m.status,
  m.client_message_id,
  m.recalled_at
FROM session_messages m
JOIN sessions s ON s.id=m.session_id
WHERE s.task_id IS NOT NULL;

DROP TABLE messages_pre_session_dedup;

CREATE INDEX idx_messages_thread ON messages(thread_id,created_at);
CREATE UNIQUE INDEX idx_messages_client_id
ON messages(thread_id,client_message_id)
WHERE client_message_id IS NOT NULL;

-- Replace the intake's Session binding with the task-scoped collaboration thread.
ALTER TABLE assignment_intakes RENAME TO assignment_intakes_pre_session_dedup;

CREATE TABLE assignment_intakes (
  assignment_id TEXT PRIMARY KEY REFERENCES assignments(id) ON DELETE CASCADE,
  task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  project_id TEXT NULL REFERENCES projects(id),
  project_memory_head TEXT NULL,
  project_memory_next_offset INTEGER NULL,
  project_memory_complete INTEGER NOT NULL DEFAULT 0 CHECK(project_memory_complete IN (0,1)),
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
    CHECK(interview_status IN ('not_started','pending','revision_requested','approved')),
  human_response TEXT NULL,
  approved_by_actor_id TEXT NULL,
  approved_at TEXT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  interview_thread_id TEXT NULL REFERENCES message_threads(id),
  conversation_state TEXT NOT NULL DEFAULT 'not_started'
    CHECK(conversation_state IN ('not_started','waiting_for_agent','waiting_for_human','converged')),
  interview_started_at TEXT NULL,
  final_summary_message_id TEXT NULL REFERENCES messages(id),
  confirmation_message_id TEXT NULL REFERENCES messages(id),
  converged_at TEXT NULL
);

INSERT INTO assignment_intakes(
  assignment_id,task_id,agent_instance_id,project_id,
  project_memory_head,project_memory_next_offset,project_memory_complete,project_memory_read_at,
  context_package_id,context_revision_id,context_package_read_at,
  understanding,constraints_json,plan_json,questions_json,unresolved_questions_json,
  interview_status,human_response,approved_by_actor_id,approved_at,created_at,updated_at,
  interview_thread_id,conversation_state,interview_started_at,
  final_summary_message_id,confirmation_message_id,converged_at
)
SELECT
  assignment_id,task_id,agent_instance_id,project_id,
  project_memory_head,project_memory_next_offset,project_memory_complete,project_memory_read_at,
  context_package_id,context_revision_id,context_package_read_at,
  understanding,constraints_json,plan_json,questions_json,unresolved_questions_json,
  interview_status,human_response,approved_by_actor_id,approved_at,created_at,updated_at,
  interview_session_id,conversation_state,interview_started_at,
  final_summary_message_id,confirmation_message_id,converged_at
FROM assignment_intakes_pre_session_dedup;

DROP TABLE assignment_intakes_pre_session_dedup;

CREATE INDEX idx_assignment_intakes_task_agent
ON assignment_intakes(task_id,agent_instance_id,updated_at DESC);
CREATE INDEX idx_assignment_intakes_interview_thread
ON assignment_intakes(interview_thread_id);

-- Task messages now use collaboration Messages. Legacy unscoped Session messages stay
-- in the historical Session tables, but they no longer participate in active delivery.
ALTER TABLE agent_deliveries RENAME TO agent_deliveries_pre_session_dedup;

CREATE TABLE agent_deliveries (
  id TEXT PRIMARY KEY,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  task_id TEXT REFERENCES tasks(id),
  kind TEXT NOT NULL CHECK(kind IN ('task_message','launch_instruction')),
  source_id TEXT NOT NULL,
  payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
  status TEXT NOT NULL DEFAULT 'queued' CHECK(status IN ('queued','claimed','delivered')),
  claimed_by_launch_attempt_id TEXT REFERENCES launch_attempts(id),
  claimed_at TEXT,
  delivered_by TEXT,
  delivered_at TEXT,
  created_at TEXT NOT NULL,
  UNIQUE(kind,source_id)
);

INSERT INTO agent_deliveries(
  id,agent_instance_id,task_id,kind,source_id,payload_json,status,
  claimed_by_launch_attempt_id,claimed_at,delivered_by,delivered_at,created_at
)
SELECT
  id,
  agent_instance_id,
  task_id,
  CASE WHEN kind='session_message' THEN 'task_message' ELSE kind END,
  source_id,
  CASE
    WHEN kind='session_message'
      THEN json_set(
        json_remove(payload_json,'$.session_id'),
        '$.thread_id',
        json_extract(payload_json,'$.session_id')
      )
    ELSE payload_json
  END,
  CASE
    WHEN status='claimed' AND claimed_by_launch_attempt_id IS NULL THEN 'queued'
    ELSE status
  END,
  claimed_by_launch_attempt_id,
  CASE
    WHEN status='claimed' AND claimed_by_launch_attempt_id IS NULL THEN NULL
    ELSE claimed_at
  END,
  delivered_by,
  delivered_at,
  created_at
FROM agent_deliveries_pre_session_dedup
WHERE kind='launch_instruction'
   OR (kind='session_message' AND task_id IS NOT NULL);

DROP TABLE agent_deliveries_pre_session_dedup;

CREATE INDEX idx_agent_deliveries_agent_queue
ON agent_deliveries(agent_instance_id,status,created_at,id);
CREATE INDEX idx_agent_deliveries_task_queue
ON agent_deliveries(agent_instance_id,task_id,status,created_at,id);
CREATE INDEX idx_agent_deliveries_claim
ON agent_deliveries(claimed_by_launch_attempt_id,status);

-- SessionRuntime credentials have no active owner after Session retirement.
-- Keep bridge and Run credentials; discard obsolete runtime credentials.
ALTER TABLE agent_credentials RENAME TO agent_credentials_pre_session_dedup;

CREATE TABLE agent_credentials (
  id TEXT PRIMARY KEY,
  agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
  token_hash TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK(kind IN ('runtime','bridge')),
  label TEXT NOT NULL,
  run_id TEXT REFERENCES runs(id),
  expires_at TEXT NOT NULL,
  revoked_at TEXT,
  created_at TEXT NOT NULL,
  CHECK(
    (kind='runtime' AND run_id IS NOT NULL)
    OR (kind='bridge' AND run_id IS NULL)
  )
);

INSERT INTO agent_credentials(
  id,agent_instance_id,token_hash,kind,label,run_id,expires_at,revoked_at,created_at
)
SELECT
  id,agent_instance_id,token_hash,kind,label,run_id,expires_at,revoked_at,created_at
FROM agent_credentials_pre_session_dedup
WHERE kind IN ('runtime','bridge');

DROP TABLE agent_credentials_pre_session_dedup;

CREATE INDEX idx_agent_credentials_agent
ON agent_credentials(agent_instance_id,kind,created_at DESC);
CREATE INDEX idx_agent_credentials_active_hash
ON agent_credentials(token_hash,expires_at)
WHERE revoked_at IS NULL;
CREATE INDEX idx_agent_credentials_run
ON agent_credentials(run_id)
WHERE run_id IS NOT NULL;

-- Use one unambiguous name for the provider-owned conversation/thread/session handle.
ALTER TABLE runs
RENAME COLUMN external_session_ref TO provider_conversation_ref;

UPDATE runs
SET provider_conversation_ref = COALESCE(
  provider_conversation_ref,
  (
    SELECT a.external_session_ref
    FROM launch_attempts a
    WHERE a.run_id=runs.id AND a.external_session_ref IS NOT NULL
    ORDER BY COALESCE(a.ended_at,a.created_at) DESC,a.id DESC
    LIMIT 1
  )
);

DROP INDEX IF EXISTS idx_launch_attempts_session;
ALTER TABLE launch_attempts DROP COLUMN session_id;
ALTER TABLE launch_attempts DROP COLUMN external_session_ref;

ALTER TABLE session_runtime_attempts
RENAME COLUMN provider_session_ref TO provider_conversation_ref;

-- Existing Sessions remain as immutable historical data only.
UPDATE sessions SET status='archived' WHERE status='open';
