use crate::Id;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPackage {
    pub id: Id,
    pub work_item_id: Id,
    pub objective: String,
    pub summary: Option<Value>,
    pub context_snapshot_id: Option<Id>,
    pub memory_refs: Vec<String>,
    pub decision_refs: Vec<Id>,
    pub artifact_refs: Vec<Id>,
    pub changed_files: Vec<String>,
    pub verified_results: Vec<String>,
    pub blockers: Vec<String>,
    pub unresolved_questions: Vec<String>,
    pub next_action: String,
    pub source_run_id: Option<Id>,
    pub source_agent_id: Option<Id>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateContextPackage {
    #[serde(default)]
    pub work_item_id: Id,
    #[serde(default)]
    pub objective: String,
    #[serde(default)]
    pub summary: Option<Value>,
    #[serde(default)]
    pub context_snapshot_id: Option<Id>,
    #[serde(default)]
    pub memory_refs: Vec<String>,
    #[serde(default)]
    pub decision_refs: Vec<Id>,
    #[serde(default)]
    pub artifact_refs: Vec<Id>,
    #[serde(default)]
    pub changed_files: Vec<String>,
    #[serde(default)]
    pub verified_results: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub unresolved_questions: Vec<String>,
    #[serde(default)]
    pub next_action: String,
    #[serde(default)]
    pub source_run_id: Option<Id>,
    #[serde(default)]
    pub source_agent_id: Option<Id>,
}
