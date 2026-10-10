use crate::DomainError;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerificationMode {
    #[default]
    SelfAttested,
    DeterministicCheck,
    ArtifactCheck,
    IndependentReview,
    HumanReview,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCriterion {
    pub id: String,
    pub requirement: String,
    #[serde(default)]
    pub verification: VerificationMode,
    #[serde(default)]
    pub allow_not_applicable: bool,
    #[serde(default)]
    pub requires_independent_review: bool,
    #[serde(default)]
    pub required_artifact_kinds: Vec<String>,
    pub check: Option<AcceptanceCheck>,
    pub artifact_check: Option<ArtifactCheck>,
    /// Retained only when importing an existing context-based contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legacy_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactCheck {
    pub path: String,
    pub machine: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCheck {
    pub command: String,
    pub machine: String,
    pub cwd: String,
    /// Publisher declaration that the check has no external write effects.
    /// This is not a shell sandbox. The existing Run runtime policy still applies.
    pub read_only: bool,
    #[serde(default = "check_timeout")]
    pub timeout_s: u32,
}
fn check_timeout() -> u32 {
    30
}

pub fn validate_acceptance(criteria: &[AcceptanceCriterion]) -> Result<(), DomainError> {
    let mut ids = std::collections::HashSet::new();
    if criteria.len() > 100 {
        return Err(DomainError::InvalidInput("at most 100 criteria".into()));
    }
    for c in criteria {
        if c.id.is_empty()
            || c.id.len() > 100
            || !c
                .id
                .chars()
                .all(|v| v.is_ascii_alphanumeric() || "-_.".contains(v))
            || !ids.insert(&c.id)
            || c.requirement.trim().is_empty()
        {
            return Err(DomainError::InvalidInput(
                "criteria need unique stable IDs and nonempty requirements".into(),
            ));
        }
        if c.legacy_path.is_some() {
            return Err(DomainError::InvalidInput(
                "legacy_path is reserved for migration".into(),
            ));
        }
        if c.verification == VerificationMode::ArtifactCheck {
            let Some(a) = &c.artifact_check else {
                return Err(DomainError::InvalidInput(
                    "artifact_check requires path and machine".into(),
                ));
            };
            if !a.path.starts_with('/') || a.machine.trim().is_empty() {
                return Err(DomainError::InvalidInput(
                    "artifact check requires absolute path and machine".into(),
                ));
            }
        } else if c.artifact_check.is_some() {
            return Err(DomainError::InvalidInput(
                "artifact_check only applies to artifact verification".into(),
            ));
        }
        if c.verification == VerificationMode::DeterministicCheck {
            let Some(check) = &c.check else {
                return Err(DomainError::InvalidInput(
                    "deterministic_check requires check".into(),
                ));
            };
            if !check.read_only
                || check.command.trim().is_empty()
                || check.command.len() > 8192
                || check.machine.trim().is_empty()
                || !check.cwd.starts_with('/')
                || !(1..=30).contains(&check.timeout_s)
            {
                return Err(DomainError::InvalidInput("checks require a read-only command, machine, absolute cwd and timeout 1..30 seconds".into()));
            }
        } else if c.check.is_some() {
            return Err(DomainError::InvalidInput(
                "check only applies to deterministic_check".into(),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CriterionStatus {
    pub criterion_id: String,
    pub verdict: String,
    pub missing_evidence: Vec<String>,
    pub receipt_ids: Vec<String>,
}

pub fn import_legacy_acceptance(constraints: &Value) -> Vec<AcceptanceCriterion> {
    crate::completion_criteria(constraints)
        .into_iter()
        .enumerate()
        .map(|(i, c)| AcceptanceCriterion {
            id: format!("legacy-{}", i + 1),
            requirement: c
                .requirement
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| c.requirement.to_string()),
            verification: VerificationMode::SelfAttested,
            allow_not_applicable: false,
            requires_independent_review: false,
            required_artifact_kinds: vec![],
            check: None,
            artifact_check: None,
            legacy_path: Some(c.path),
        })
        .collect()
}
