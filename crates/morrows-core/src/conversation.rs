use crate::Id;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateConversation {
    pub agent_instance_id: Id,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub id: Id,
    pub agent_instance_id: Id,
    pub title: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummary {
    pub id: Id,
    pub agent_instance_id: Id,
    pub agent_name: String,
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
pub struct ConversationMessage {
    pub id: Id,
    pub conversation_id: Id,
    pub author_type: String,
    pub author_agent_instance_id: Option<Id>,
    pub body: String,
    /// Conversation processing state. Human messages remain queued until an Agent reply.
    pub status: String,
    /// Runtime delivery state is independent from reply/processing state.
    pub delivery_status: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationHistory {
    pub conversation: Conversation,
    pub messages: Vec<ConversationMessage>,
    pub has_more: bool,
    pub next_before: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConversationReply {
    pub conversation_id: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ConversationHistoryRequest {
    pub conversation_id: String,
    #[serde(default)]
    pub before_message_id: Option<String>,
    #[serde(default)]
    pub after_message_id: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationSummaryRevision {
    pub id: Id,
    pub conversation_id: Id,
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
pub struct CreateConversationSummaryRevision {
    #[serde(default)]
    pub conversation_id: Id,
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
