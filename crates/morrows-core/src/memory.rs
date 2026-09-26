use crate::Id;
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: Id,
    pub scope_type: String,
    pub project_id: Option<Id>,
    pub agent_instance_id: Option<Id>,
    pub task_id: Option<Id>,
    pub title: String,
    pub content: Value,
    pub source_kind: String,
    pub source_ref: Option<String>,
    pub visibility: String,
    pub supersedes_memory_id: Option<Id>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub provenance: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PublishProjectMemory {
    #[schemars(with = "String")]
    pub task_id: Id,
    /// Stable caller-generated key for this publication; retry with the same payload.
    pub idempotency_key: String,
    /// Optional unused UUID for a new document; keeps a native file's identity stable.
    /// Revisions use supersedes_memory_id instead. Omission preserves legacy receipts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub new_memory_id: Option<Id>,
    /// Pin the project actually read; checked inside the publication transaction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub expected_project_id: Option<Id>,
    pub title: String,
    pub content: Value,
    /// reported, verified, hypothesis, or unverified; an author claim, not server certification.
    pub verification_status: String,
    pub basis: String,
    #[schemars(with = "String")]
    pub context_revision_id: Id,
    #[serde(default)]
    pub artifact_ids: Vec<String>,
    #[serde(default)]
    pub decision_ids: Vec<String>,
    #[schemars(with = "Option<String>")]
    pub supersedes_memory_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMemoryEntry {
    pub scope_type: String,
    pub project_id: Option<Id>,
    pub agent_instance_id: Option<Id>,
    pub task_id: Option<Id>,
    pub title: String,
    pub content: Value,
    pub source_kind: String,
    pub source_ref: Option<String>,
    #[serde(default = "default_visibility")]
    pub visibility: String,
    pub supersedes_memory_id: Option<Id>,
}

fn default_visibility() -> String {
    "shared".into()
}
