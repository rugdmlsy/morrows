UPDATE launch_profiles
SET model = 'gpt-5.6-luna'
WHERE adapter = 'codex_cli'
  AND (model IS NULL OR trim(model) = '');
