use crate::Id;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSession {
    pub agent_instance_id: Id,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: Id,
    pub agent_instance_id: Id,
    pub project_id: Option<Id>,
    pub task_id: Option<Id>,
    pub title: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: Id,
    pub agent_instance_id: Id,
    pub agent_name: String,
    pub project_id: Option<Id>,
    pub project_name: Option<String>,
    pub task_id: Option<Id>,
    pub task_title: Option<String>,
    pub title: String,
    pub status: String,
    pub message_count: i64,
    /// Human messages that have not yet received an Agent reply.
    pub queued_count: i64,
    /// Human messages whose durable delivery has not yet reached an Agent runtime/bridge.
    pub undelivered_count: i64,
    pub last_message_preview: Option<String>,
    pub last_message_author_type: Option<String>,
    pub last_message_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMessage {
    pub id: Id,
    pub session_id: Id,
    pub author_type: String,
    pub author_agent_instance_id: Option<Id>,
    pub body: String,
    /// Stable client-generated idempotency key for human messages.
    pub client_message_id: Option<String>,
    /// A recalled message remains durable for audit but is excluded from delivery semantics.
    pub recalled_at: Option<DateTime<Utc>>,
    /// Session processing state. Human messages remain queued until an Agent reply.
    pub status: String,
    /// Runtime delivery state is independent from reply/processing state.
    pub delivery_status: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHistory {
    pub session: Session,
    pub messages: Vec<SessionMessage>,
    pub has_more: bool,
    pub next_before: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateSessionScope {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub project_id: Option<Id>,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub task_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct StartSessionRuntime {
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub launch_profile_id: Option<Id>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRuntimeAttempt {
    pub id: Id,
    pub session_id: Id,
    pub agent_instance_id: Id,
    pub account_id: Option<Id>,
    pub launch_profile_id: Id,
    pub adapter: String,
    pub status: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub cwd: Option<String>,
    pub provider_session_ref: Option<String>,
    pub pid: Option<i64>,
    pub exit_code: Option<i64>,
    pub stdout_path: Option<String>,
    pub stderr_path: Option<String>,
    pub error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionReply {
    pub session_id: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SessionHistoryRequest {
    pub session_id: String,
    #[serde(default)]
    pub before_message_id: Option<String>,
    #[serde(default)]
    pub after_message_id: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummaryRevision {
    pub id: Id,
    pub session_id: Id,
    pub previous_revision_id: Option<Id>,
    pub covers_until_message_id: Option<Id>,
    pub goal: String,
    pub current_state: String,
    pub important_findings: serde_json::Value,
    pub decisions: serde_json::Value,
    pub blockers: serde_json::Value,
    pub unresolved_questions: serde_json::Value,
    pub next_steps: serde_json::Value,
    pub deterministic_facts: serde_json::Value,
    pub created_by: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateSessionSummaryRevision {
    #[serde(default)]
    pub session_id: Id,
    #[serde(default)]
    pub previous_revision_id: Option<Id>,
    #[serde(default)]
    pub covers_until_message_id: Option<Id>,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub current_state: String,
    #[serde(default = "empty_array_json")]
    pub important_findings: serde_json::Value,
    #[serde(default = "empty_array_json")]
    pub decisions: serde_json::Value,
    #[serde(default = "empty_array_json")]
    pub blockers: serde_json::Value,
    #[serde(default = "empty_array_json")]
    pub unresolved_questions: serde_json::Value,
    #[serde(default = "empty_array_json")]
    pub next_steps: serde_json::Value,
    #[serde(default = "empty_object_json")]
    pub deterministic_facts: serde_json::Value,
    #[serde(default = "default_summary_creator")]
    pub created_by: String,
}

fn empty_array_json() -> serde_json::Value {
    serde_json::json!([])
}

fn empty_object_json() -> serde_json::Value {
    serde_json::json!({})
}

fn default_summary_creator() -> String {
    "system".into()
}
