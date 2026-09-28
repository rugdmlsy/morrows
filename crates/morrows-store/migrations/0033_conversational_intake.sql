ALTER TABLE assignment_intakes
ADD COLUMN interview_session_id TEXT NULL REFERENCES sessions(id);

ALTER TABLE assignment_intakes
ADD COLUMN conversation_state TEXT NOT NULL DEFAULT 'not_started'
CHECK (conversation_state IN ('not_started','waiting_for_agent','waiting_for_human','converged'));

ALTER TABLE assignment_intakes
ADD COLUMN interview_started_at TEXT NULL;

ALTER TABLE assignment_intakes
ADD COLUMN final_summary_message_id TEXT NULL REFERENCES session_messages(id);

ALTER TABLE assignment_intakes
ADD COLUMN confirmation_message_id TEXT NULL REFERENCES session_messages(id);

ALTER TABLE assignment_intakes
ADD COLUMN converged_at TEXT NULL;

UPDATE assignment_intakes
SET conversation_state = CASE interview_status
  WHEN 'approved' THEN 'converged'
  WHEN 'pending' THEN 'waiting_for_human'
  WHEN 'revision_requested' THEN 'waiting_for_agent'
  ELSE 'not_started'
END,
converged_at = CASE WHEN interview_status='approved' THEN approved_at ELSE NULL END;

CREATE INDEX idx_assignment_intakes_interview_session
ON assignment_intakes(interview_session_id);
