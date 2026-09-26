ALTER TABLE tasks
ADD COLUMN assignment_mode TEXT NOT NULL DEFAULT 'open'
CHECK (assignment_mode IN ('open','approval','dispatch'));

-- Existing tasks with an enabled dispatcher stay dispatcher-owned after the
-- cutover instead of unexpectedly becoming self-claimable.
UPDATE tasks
SET assignment_mode='dispatch'
WHERE EXISTS (
  SELECT 1 FROM task_dispatch_policies p
  WHERE p.task_id=tasks.id AND p.enabled=1
);

CREATE INDEX idx_tasks_assignment_mode_state
ON tasks(assignment_mode,state,priority DESC,created_at);
