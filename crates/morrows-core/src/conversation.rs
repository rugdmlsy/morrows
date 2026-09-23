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
    pub queued_count: i64,
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
    pub status: String,
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
