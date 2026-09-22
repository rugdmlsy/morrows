-- Extend instances in place: all existing UUIDs and inbound foreign keys survive.
CREATE TABLE agent_profiles (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  provider TEXT NOT NULL,
  kind TEXT NOT NULL,
  default_capabilities_json TEXT NOT NULL,
  metadata_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(provider,name)
);

CREATE TABLE accounts (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  label TEXT NOT NULL,
  external_account_ref TEXT,
  status TEXT NOT NULL,
  metadata_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(provider,label)
);

CREATE TABLE machines (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  hostname TEXT NOT NULL,
  os TEXT NOT NULL,
  arch TEXT NOT NULL,
  status TEXT NOT NULL,
  metadata_json TEXT NOT NULL,
  last_seen_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  UNIQUE(name)
);

-- One explicit legacy product; each legacy name receives its own unknown account/host.
-- Using instance UUIDs in the separate account/machine tables is deterministic and
-- does not assert that different legacy workers share a real account or machine.
INSERT INTO agent_profiles VALUES('00000000-0000-0000-0000-000000000003','legacy','legacy','legacy','[]','{"legacy":true}',strftime('%Y-%m-%dT%H:%M:%fZ','now'),strftime('%Y-%m-%dT%H:%M:%fZ','now'));
INSERT INTO accounts(id,provider,label,status,metadata_json,created_at,updated_at)
 SELECT id,'legacy',name,'unknown','{"legacy":true}',last_heartbeat_at,last_heartbeat_at FROM agent_instances;
INSERT INTO machines(id,name,hostname,os,arch,status,metadata_json,last_seen_at,created_at,updated_at)
 SELECT id,'legacy:' || id,'unknown','unknown','unknown','unknown','{"legacy":true}',last_heartbeat_at,last_heartbeat_at,last_heartbeat_at FROM agent_instances;
ALTER TABLE agent_instances ADD COLUMN profile_id TEXT REFERENCES agent_profiles(id);
ALTER TABLE agent_instances ADD COLUMN account_id TEXT REFERENCES accounts(id);
ALTER TABLE agent_instances ADD COLUMN machine_id TEXT REFERENCES machines(id);
ALTER TABLE agent_instances ADD COLUMN external_instance_ref TEXT;
ALTER TABLE agent_instances ADD COLUMN created_at TEXT;
UPDATE agent_instances SET profile_id='00000000-0000-0000-0000-000000000003',account_id=id,machine_id=id,created_at=last_heartbeat_at;

-- SQLite cannot add a required FK column to a populated table; triggers enforce
-- the post-backfill requirement without rebuilding the referenced instance table.
CREATE TRIGGER instance_identity_insert BEFORE INSERT ON agent_instances BEGIN
 SELECT CASE WHEN NEW.profile_id IS NULL OR NEW.created_at IS NULL THEN RAISE(ABORT,'instance profile and created_at required') END;
 SELECT CASE WHEN EXISTS(SELECT 1 FROM accounts a JOIN agent_profiles p ON p.id=NEW.profile_id WHERE a.id=NEW.account_id AND a.provider<>'' AND p.provider<>'' AND a.provider<>p.provider) THEN RAISE(ABORT,'account/profile provider mismatch') END;
END;
CREATE TRIGGER instance_identity_update BEFORE UPDATE ON agent_instances BEGIN
 SELECT CASE WHEN NEW.profile_id IS NULL OR NEW.created_at IS NULL THEN RAISE(ABORT,'instance profile and created_at required') END;
 SELECT CASE WHEN EXISTS(SELECT 1 FROM accounts a JOIN agent_profiles p ON p.id=NEW.profile_id WHERE a.id=NEW.account_id AND a.provider<>'' AND p.provider<>'' AND a.provider<>p.provider) THEN RAISE(ABORT,'account/profile provider mismatch') END;
END;
CREATE TABLE capacity_snapshots (
 id TEXT PRIMARY KEY,
 agent_instance_id TEXT NOT NULL REFERENCES agent_instances(id),
 status TEXT NOT NULL,
 available_slots INTEGER NOT NULL CHECK(available_slots>=0),
 active_assignments INTEGER NOT NULL CHECK(active_assignments>=0),
 active_runs INTEGER NOT NULL CHECK(active_runs>=0),
 max_concurrency INTEGER CHECK(max_concurrency>=0 AND available_slots<=max_concurrency),
 quota_state TEXT,
 details_json TEXT NOT NULL DEFAULT '{}',
 observed_at TEXT NOT NULL
);
CREATE INDEX idx_capacity_instance_observed ON capacity_snapshots(agent_instance_id,observed_at DESC);
CREATE TRIGGER capacity_no_update BEFORE UPDATE ON capacity_snapshots BEGIN
 SELECT RAISE(ABORT,'capacity snapshots are append-only');
END;
CREATE TRIGGER capacity_no_delete BEFORE DELETE ON capacity_snapshots BEGIN
 SELECT RAISE(ABORT,'capacity snapshots are append-only');
END;
