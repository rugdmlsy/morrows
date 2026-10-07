use crate::{Assignment, ContextPackage, Id, MemoryEntry, Page, Project};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const INTAKE_PHASE_CONTEXT_REVIEW: &str = "context_review";
pub const INTAKE_PHASE_HUMAN_INTERVIEW: &str = "human_interview";
pub const INTAKE_PHASE_READY: &str = "ready";
pub const INTAKE_PHASE_IMPLEMENTING: &str = "implementing";
pub const INTERVIEW_STATE_NOT_STARTED: &str = "not_started";
pub const INTERVIEW_STATE_WAITING_FOR_AGENT: &str = "waiting_for_agent";
pub const INTERVIEW_STATE_WAITING_FOR_HUMAN: &str = "waiting_for_human";
pub const INTERVIEW_STATE_CONVERGED: &str = "converged";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentIntake {
    pub assignment_id: Id,
    pub task_id: Id,
    pub agent_instance_id: Id,
    pub project_id: Option<Id>,
    pub project_memory_head: Option<String>,
    pub project_memory_next_offset: Option<i64>,
    pub project_memory_complete: bool,
    pub project_memory_read_at: Option<DateTime<Utc>>,
    pub context_package_id: Option<Id>,
    pub context_revision_id: Option<Id>,
    pub context_package_read_at: Option<DateTime<Utc>>,
    pub understanding: String,
    pub constraints: Value,
    pub plan: Value,
    pub questions: Vec<String>,
    pub unresolved_questions: Vec<String>,
    /// Legacy approval status retained for migration/backward compatibility.
    pub interview_status: String,
    pub human_response: Option<String>,
    pub approved_by_actor_id: Option<String>,
    pub approved_at: Option<DateTime<Utc>>,
    /// Durable task-scoped collaboration thread used for the multi-turn Human Interview.
    pub interview_thread_id: Option<Id>,
    /// not_started, waiting_for_agent, waiting_for_human, or converged.
    pub conversation_state: String,
    pub interview_started_at: Option<DateTime<Utc>>,
    /// Agent-authored final synthesis shown to the human before convergence.
    pub final_summary_message_id: Option<Id>,
    /// Human-authored response after the final synthesis.
    pub confirmation_message_id: Option<Id>,
    pub converged_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct TaskIntakeView {
    pub assignment: Assignment,
    pub intake: AssignmentIntake,
    pub project: Project,
    pub project_memory: Page<MemoryEntry>,
    pub context_package: ContextPackage,
    pub blockers: Vec<String>,
    pub execution_ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterviewSubmission {
    pub understanding: String,
    #[serde(default = "default_object")]
    pub constraints: Value,
    #[serde(default = "default_array")]
    pub plan: Value,
    #[serde(default)]
    pub questions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterviewFinalize {
    pub understanding: String,
    #[serde(default = "default_object")]
    pub constraints: Value,
    #[serde(default = "default_array")]
    pub plan: Value,
    #[serde(default)]
    pub unresolved_questions: Vec<String>,
    #[serde(default)]
    pub final_summary_message_id: Option<Id>,
    #[serde(default)]
    pub confirmation_message_id: Option<Id>,
}

fn default_object() -> Value {
    serde_json::json!({})
}

fn default_array() -> Value {
    serde_json::json!([])
}
