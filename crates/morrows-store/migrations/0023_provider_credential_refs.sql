ALTER TABLE accounts ADD COLUMN credential_kind TEXT;
ALTER TABLE accounts ADD COLUMN credential_ref TEXT;

-- Move legacy LaunchProfile-owned CODEX_HOME references onto the owning Account.
-- This copies only the path/reference. Provider login material itself remains outside SQLite.
UPDATE accounts
SET
  credential_kind='codex_home',
  credential_ref=(
    SELECT lp.codex_home
    FROM agent_instances ai
    JOIN launch_profiles lp ON lp.agent_instance_id=ai.id
    WHERE ai.account_id=accounts.id
      AND lp.adapter='codex_cli'
      AND lp.codex_home IS NOT NULL
      AND trim(lp.codex_home)<>''
    ORDER BY lp.created_at DESC,lp.id DESC
    LIMIT 1
  )
WHERE provider='openai'
  AND credential_ref IS NULL
  AND EXISTS(
    SELECT 1
    FROM agent_instances ai
    JOIN launch_profiles lp ON lp.agent_instance_id=ai.id
    WHERE ai.account_id=accounts.id
      AND lp.adapter='codex_cli'
      AND lp.codex_home IS NOT NULL
      AND trim(lp.codex_home)<>''
  );

-- Remove legacy metadata that claimed Morrows owned/copied provider auth files.
UPDATE accounts
SET metadata_json=json_set(
  json_remove(metadata_json,'$.auth_isolation'),
  '$.credential_source','external_reference',
  '$.credential_secret_stored',json('false')
)
WHERE provider='openai' AND credential_ref IS NOT NULL;

UPDATE launch_profiles
SET metadata_json=json_set(
  json_remove(metadata_json,'$.auth_isolation'),
  '$.credential_source','account_ref'
)
WHERE adapter='codex_cli';

-- CODEX_HOME is now Account credential state, not launch configuration.
UPDATE launch_profiles
SET codex_home=NULL
WHERE adapter='codex_cli' AND codex_home IS NOT NULL;
