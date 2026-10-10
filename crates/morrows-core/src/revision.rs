use crate::AcceptanceCriterion;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The complete, durable task contract at one effective specification version.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct TaskSpec {
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<AcceptanceCriterion>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskRevisionDraft {
    pub expected_version: i64,
    pub reason: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub acceptance_criteria: Option<Vec<AcceptanceCriterion>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskRevisionAck {
    pub revision_id: String,
    pub run_id: String,
    pub impact: String,
    pub updated_plan: Vec<String>,
}
