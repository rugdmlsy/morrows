use crate::Id;
use chrono::{DateTime, Utc};
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
