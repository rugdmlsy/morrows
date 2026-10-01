use super::*;
use schemars::JsonSchema;

pub const DEFAULT_CODEX_MODEL: &str = "gpt-5.6-luna";

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
pub struct RunRuntimeBinding {
    pub run_id: Id,
    #[serde(alias = "logical_session_id")]
    pub runtime_scope_id: String,
    pub capability_id: Option<String>,
    pub restart_deadline_at: Option<DateTime<Utc>>,
}

/// Backward-compatible Rust name for callers created before the morrow-runtime rename.
pub type RunLsmBinding = RunRuntimeBinding;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeJobTerminalEvent {
    pub event_id: String,
    pub job_id: String,
    pub source_machine: String,
    #[serde(default, alias = "logical_session_id")]
    pub runtime_scope_id: Option<String>,
    pub attempt: i64,
    pub status: String,
    pub exit_code: Option<i64>,
    pub completed_at: DateTime<Utc>,
    pub terminal_reason: String,
    pub summary_ref: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LsmJobTerminalEvent {
    pub event_id: String,
    pub job_id: String,
    pub source_machine: String,
    pub logical_session_id: Option<String>,
    pub attempt: i64,
    pub status: String,
    pub exit_code: Option<i64>,
    pub completed_at: DateTime<Utc>,
    pub terminal_reason: String,
    pub summary_ref: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
}

impl From<LsmJobTerminalEvent> for RuntimeJobTerminalEvent {
    fn from(event: LsmJobTerminalEvent) -> Self {
        Self {
            event_id: event.event_id,
            job_id: event.job_id,
            source_machine: event.source_machine,
            runtime_scope_id: event.logical_session_id,
            attempt: event.attempt,
            status: event.status,
            exit_code: event.exit_code,
            completed_at: event.completed_at,
            terminal_reason: event.terminal_reason,
            summary_ref: event.summary_ref,
            result: event.result,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RegisterRunJobWait {
    #[schemars(with = "String")]
    pub run_id: Id,
    pub source_machine: String,
    pub job_id: String,
    pub resume_plan: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunJobWait {
    pub id: Id,
    pub run_id: Id,
    pub source_machine: String,
    pub job_id: String,
    #[serde(alias = "logical_session_id")]
    pub runtime_scope_id: String,
    pub status: String,
    pub resume_mode: Option<String>,
    pub resume_plan: String,
    pub reason: String,
    pub terminal_event_id: Option<String>,
    pub terminal_status: Option<String>,
    pub terminal_reason: Option<String>,
    pub resume_launch_attempt_id: Option<Id>,
    pub summary_ref: Option<String>,
    pub result: Option<Value>,
    pub registered_at: DateTime<Utc>,
    pub ready_at: Option<DateTime<Utc>>,
    pub queued_at: Option<DateTime<Utc>>,
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

#[cfg(test)]
mod job_wait_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn lsm_job_terminal_event_http_json_uses_rfc3339_timestamp() {
        let event: LsmJobTerminalEvent = serde_json::from_value(json!({
            "event_id": "job-finish:test:1",
            "job_id": "job-test",
            "source_machine": "morrow-node-01",
            "logical_session_id": "s_test",
            "attempt": 1,
            "status": "succeeded",
            "exit_code": 0,
            "completed_at": "1970-01-01T00:02:03Z",
            "terminal_reason": "process exited successfully",
            "summary_ref": "work/report.json",
            "result": {"records": 147}
        }))
        .unwrap();

        assert_eq!(event.completed_at.timestamp(), 123);
        assert_eq!(event.logical_session_id.as_deref(), Some("s_test"));
    }
}
