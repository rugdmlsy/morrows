-- A retry of an authenticated transfer returns the same accepted Assignment/Run.
-- The actor namespace prevents two identities using the same key to alias receipts.
CREATE TABLE executor_takeover_requests (
    actor TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    successor_agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
    reason TEXT NOT NULL,
    force INTEGER NOT NULL CHECK(force IN (0,1)),
    assignment_id TEXT NOT NULL REFERENCES assignments(id),
    run_id TEXT NOT NULL REFERENCES runs(id),
    created_at TEXT NOT NULL,
    PRIMARY KEY(actor,idempotency_key)
);
CREATE INDEX idx_executor_takeover_receipts_task
    ON executor_takeover_requests(task_id,created_at DESC);
