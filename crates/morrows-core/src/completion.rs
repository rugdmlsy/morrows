use crate::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionCriterion {
    /// JSON pointer into the current context's constraints, not a rewritten label.
    pub path: String,
    pub requirement: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompletionEvidence {
    pub criterion_path: String,
    /// Only passed satisfies completion. Preserve failed/unknown work in checkpoints.
    pub status: String,
    pub rationale: String,
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompletionReport {
    pub context_revision_id: String,
    pub checks: Vec<CompletionEvidence>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionReadiness {
    pub run_id: Id,
    pub task_id: Id,
    pub context_revision_id: Option<Id>,
    pub criteria: Vec<CompletionCriterion>,
    pub completion_required: bool,
    pub ready: bool,
    pub blockers: Vec<String>,
    pub completion_template: Value,
    pub verification_limit: String,
}

/// Keep each declared array item/object field intact, including rich structured
/// requirements. Stable JSON pointers bind a report to the exact saved criteria;
/// this is a structural contract, not an interpretation of their truth.
pub fn completion_criteria(constraints: &Value) -> Vec<CompletionCriterion> {
    let mut criteria = Vec::new();
    for key in ["acceptance_criteria", "freeze_requires"] {
        let Some(value) = constraints.get(key) else {
            continue;
        };
        let entries: Vec<_> = match value {
            Value::Array(items) => items
                .iter()
                .enumerate()
                .map(|(i, value)| (format!("/{key}/{i}"), value))
                .collect(),
            Value::Object(items) => items
                .iter()
                .map(|(name, value)| {
                    (
                        format!("/{key}/{}", name.replace('~', "~0").replace('/', "~1")),
                        value,
                    )
                })
                .collect(),
            Value::Null => Vec::new(),
            Value::String(text) if text.trim().is_empty() => Vec::new(),
            _ => vec![(format!("/{key}"), value)],
        };
        criteria.extend(
            entries
                .into_iter()
                .map(|(path, requirement)| CompletionCriterion {
                    path,
                    requirement: requirement.clone(),
                }),
        );
    }
    criteria
}
