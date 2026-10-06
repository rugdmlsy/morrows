UPDATE sessions
SET status = 'archived',
    updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
WHERE status = 'open'
  AND id IN (
    SELECT ai.interview_session_id
    FROM assignment_intakes ai
    JOIN assignments a ON a.id = ai.assignment_id
    WHERE ai.interview_session_id IS NOT NULL
      AND (ai.interview_status = 'approved' OR ai.conversation_state = 'converged')
      AND (a.phase = 'implementing' OR a.status != 'active')
  );
