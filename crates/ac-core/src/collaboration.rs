use crate::{ContextRevision, Id};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

fn default_artifact_kind() -> String {
    "other".into()
}
fn default_message_type() -> String {
    "note".into()
}
fn default_message_status() -> String {
    "sent".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateArtifact {
    pub title: String,
    pub uri: String,
    #[serde(default = "default_artifact_kind")]
    pub kind: String,
    #[serde(default)]
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: Id,
    pub task_id: Id,
    pub created_by: Id,
    #[serde(flatten)]
    pub content: CreateArtifact,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateDecision {
    pub title: String,
    pub rationale: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub id: Id,
    pub task_id: Id,
    pub created_by: Id,
    #[serde(flatten)]
    pub content: CreateDecision,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateThread {
    pub title: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageThread {
    pub id: Id,
    pub task_id: Id,
    pub created_by: Id,
    pub title: String,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateMessage {
    pub body: String,
    #[serde(default = "default_message_type")]
    pub message_type: String,
    #[serde(default)]
    pub recipient_agent_instance_id: Option<String>,
    #[serde(default)]
    pub recipient_role: Option<String>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub requires_response: bool,
    #[serde(default = "default_message_status")]
    pub status: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: Id,
    pub thread_id: Id,
    pub created_by: Id,
    pub body: String,
    #[serde(default = "default_message_type")]
    pub message_type: String,
    #[serde(default)]
    pub recipient_agent_instance_id: Option<String>,
    #[serde(default)]
    pub recipient_role: Option<String>,
    #[serde(default)]
    pub reply_to_message_id: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    #[serde(default)]
    pub requires_response: bool,
    #[serde(default = "default_message_status")]
    pub status: String,

    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateHandoff {
    pub summary: String,
    pub completed: Vec<String>,
    pub remaining: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub artifact_ids: Vec<String>,
    #[serde(default)]
    pub decision_ids: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Handoff {
    pub id: Id,
    pub task_id: Id,
    pub source_run_id: Id,
    pub status: String,
    pub accepted_by_run_id: Option<Id>,
    pub created_by: Id,
    pub context_revision_id: Id,
    #[serde(flatten)]
    pub content: CreateHandoff,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDependency {
    pub task_id: Id,
    pub depends_on_task_id: Id,
    pub created_by: Id,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Collaboration {
    pub handoffs: Vec<Handoff>,
    pub artifacts: Vec<Artifact>,
    pub decisions: Vec<Decision>,
    pub threads: Vec<MessageThread>,
    pub messages: Vec<Message>,
    pub dependencies: Vec<TaskDependency>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffContext {
    pub handoff: Handoff,
    pub context: ContextRevision,
    pub artifacts: Vec<Artifact>,
    pub decisions: Vec<Decision>,
}
