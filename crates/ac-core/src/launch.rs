use super::*;
use schemars::JsonSchema;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RegisterLaunchProfile {
    pub name: String,
    pub adapter: String,
    #[schemars(with = "String")]
    pub agent_instance_id: Id,
    pub program: String,
    pub codex_home: Option<String>,
    pub default_cwd: Option<String>,
    pub model: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "empty_object")]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchProfile {
    pub id: Id,
    pub name: String,
    pub adapter: String,
    pub agent_instance_id: Id,
    pub program: String,
    pub codex_home: Option<String>,
    pub default_cwd: Option<String>,
    pub model: Option<String>,
    pub enabled: bool,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnqueueLaunch {
    #[schemars(with = "String")]
    pub assignment_id: Id,
    #[schemars(with = "String")]
    pub launch_profile_id: Id,
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchAttempt {
    pub id: Id,
    pub assignment_id: Id,
    pub task_id: Id,
    pub agent_instance_id: Id,
    pub launch_profile_id: Id,
    pub run_id: Option<Id>,
    pub job_id: Option<Id>,
    pub status: String,
    pub cwd: Option<String>,
    pub external_session_ref: Option<String>,
    pub pid: Option<i64>,
    pub exit_code: Option<i64>,
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaimedLaunchJob {
    pub job_id: Id,
    pub attempt: LaunchAttempt,
    pub profile: LaunchProfile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchExecution {
    pub attempt: LaunchAttempt,
    pub run: Run,
    pub profile: LaunchProfile,
    pub task: Task,
    pub context: Option<ContextRevision>,
}

fn default_enabled() -> bool {
    true
}
