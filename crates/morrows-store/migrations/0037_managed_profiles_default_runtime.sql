-- Migration 0036 preserved the old implicit local backend for pre-cutover profiles.
-- Managed Morrows CLI profiles should instead follow the new default execution policy.
UPDATE launch_profiles
SET metadata_json = json_set(
  metadata_json,
  '$.execution_backend', 'morrow_runtime',
  '$.execution_backend_migrated_by', '0037_managed_profiles_default_runtime'
)
WHERE adapter IN ('codex_cli','codebuddy_cli')
  AND json_extract(metadata_json,'$.execution_backend') = 'local'
  AND (
    json_extract(metadata_json,'$.managed_by') = 'morrows'
    OR json_extract(metadata_json,'$.credential_source') = 'account_ref'
  );
