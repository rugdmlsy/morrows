CREATE TABLE dispatch_scheduler_settings (
  role TEXT PRIMARY KEY,
  enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
  interval_seconds INTEGER NOT NULL DEFAULT 2 CHECK(interval_seconds BETWEEN 1 AND 300),
  auto_launch INTEGER NOT NULL DEFAULT 1 CHECK(auto_launch IN (0,1)),
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

INSERT INTO dispatch_scheduler_settings(
  role,enabled,interval_seconds,auto_launch,created_at,updated_at
) VALUES(
  'executor',0,2,1,
  strftime('%Y-%m-%dT%H:%M:%fZ','now'),
  strftime('%Y-%m-%dT%H:%M:%fZ','now')
);
