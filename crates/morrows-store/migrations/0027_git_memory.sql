-- Git owns published versions after explicit, equivalence-checked cutover.
-- SQL retains the original rows as a rebuildable projection and audit evidence.
CREATE TABLE memory_git_domains (
    domain TEXT PRIMARY KEY,
    backend TEXT NOT NULL CHECK(backend IN ('sql','git')),
    repository TEXT NOT NULL,
    ref_name TEXT NOT NULL UNIQUE,
    indexed_commit TEXT,
    verified_commit TEXT,
    verified_count INTEGER,
    updated_at TEXT NOT NULL
);
CREATE TABLE memory_git_operations (
    id TEXT PRIMARY KEY,
    domain TEXT NOT NULL REFERENCES memory_git_domains(domain),
    kind TEXT NOT NULL CHECK(kind IN ('mirror','publication')),
    actor_id TEXT,
    retry_key TEXT,
    request_json TEXT NOT NULL CHECK(json_valid(request_json)),
    payload_json TEXT NOT NULL CHECK(json_valid(payload_json)),
    base_commit TEXT,
    result_commit TEXT,
    state TEXT NOT NULL CHECK(state IN ('prepared','indexed','conflict')),
    created_at TEXT NOT NULL,
    UNIQUE(actor_id,retry_key)
);
CREATE INDEX memory_git_pending ON memory_git_operations(state,domain);
