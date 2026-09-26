use super::*;
use schemars::JsonSchema;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RegisterLaunchProfile {
    pub name: String,
    pub adapter: String,
    #[schemars(with = "String")]
    pub agent_instance_id: Id,
    #[serde(default)]
    pub program: String,
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
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub resume_from_attempt_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchAttempt {
    pub id: Id,
    pub assignment_id: Id,
    pub task_id: Id,
    pub agent_instance_id: Id,
    pub launch_profile_id: Id,
    pub run_id: Option<Id>,
    pub session_id: Option<Id>,
    pub job_id: Option<Id>,
    pub resume_from_attempt_id: Option<Id>,
    pub restart_run_id: Option<Id>,
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
pub struct RunLsmBinding {
    pub run_id: Id,
    pub logical_session_id: String,
    pub capability_id: Option<String>,
    pub restart_deadline_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AttachExecutionEvidence {
    #[schemars(with = "Option<String>")]
    pub event_id: Option<Id>,
    pub kind: String,
    pub reference: String,
    pub start_seq: Option<i64>,
    pub end_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunExecutionEvidence {
    pub id: Id,
    pub run_id: Id,
    pub event_id: Option<Id>,
    pub kind: String,
    pub reference: String,
    pub start_seq: Option<i64>,
    pub end_seq: Option<i64>,
    pub created_at: DateTime<Utc>,
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
    pub account: Option<Account>,
    pub task: Task,
    pub context: Option<ContextRevision>,
    pub instructions: Vec<LaunchInstruction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AcceptExternalLaunch {
    #[schemars(with = "String")]
    pub launch_attempt_id: Id,
    pub external_session_ref: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SendLaunchInstruction {
    #[schemars(with = "String")]
    pub launch_attempt_id: Id,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchInstruction {
    pub id: Id,
    pub launch_attempt_id: Id,
    pub actor_type: String,
    pub actor_id: String,
    pub body: String,
    pub created_at: DateTime<Utc>,
}

fn default_enabled() -> bool {
    true
}
