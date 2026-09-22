use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use uuid::Uuid;

pub type Id = Uuid;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Backlog,
    Ready,
    InProgress,
    Review,
    Blocked,
    Done,
    Cancelled,
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Backlog => "backlog",
            Self::Ready => "ready",
            Self::InProgress => "in_progress",
            Self::Review => "review",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        };
        f.write_str(s)
    }
}

impl std::str::FromStr for TaskState {
    type Err = DomainError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "backlog" => Ok(Self::Backlog),
            "ready" => Ok(Self::Ready),
            "in_progress" => Ok(Self::InProgress),
            "review" => Ok(Self::Review),
            "blocked" => Ok(Self::Blocked),
            "done" => Ok(Self::Done),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(DomainError::InvalidState(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: Id,
    pub project_id: Option<Id>,
    pub title: String,
    pub description: String,
    pub owner_actor_id: String,
    pub state: TaskState,
    pub priority: i32,
    pub current_context_revision_id: Option<Id>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTask {
    pub project_id: Option<Id>,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default = "default_owner")]
    pub owner_actor_id: String,
    #[serde(default = "default_task_state")]
    pub state: TaskState,
    #[serde(default)]
    pub priority: i32,
}

fn default_owner() -> String {
    "human:local".into()
}
fn default_task_state() -> TaskState {
    TaskState::Ready
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentInstance {
    pub id: Id,
    pub name: String,
    pub status: String,
    pub capabilities: Vec<String>,
    pub last_heartbeat_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Assignment {
    pub id: Id,
    pub task_id: Id,
    pub role: String,
    pub agent_instance_id: Id,
    pub status: String,
    pub acquired_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub renewed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Run {
    pub id: Id,
    pub task_id: Id,
    pub assignment_id: Id,
    pub agent_instance_id: Id,
    pub external_session_ref: Option<String>,
    pub status: String,
    pub stop_reason: Option<String>,
    pub checkpoint: Option<Value>,
    pub result: Option<Value>,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRevision {
    pub id: Id,
    pub task_id: Id,
    pub version: i64,
    pub parent_revision_id: Option<Id>,
    pub goal: String,
    pub background: String,
    pub constraints: Value,
    pub current_summary: String,
    pub created_by_actor_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateContextRevision {
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub background: String,
    #[serde(default = "empty_object")]
    pub constraints: Value,
    #[serde(default)]
    pub current_summary: String,
    #[serde(default = "default_owner")]
    pub created_by_actor_id: String,
}

fn empty_object() -> Value {
    serde_json::json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub id: Id,
    pub actor_type: String,
    pub actor_id: String,
    pub entity_type: String,
    pub entity_id: Id,
    pub event_type: String,
    pub correlation_id: Option<String>,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, thiserror::Error)]
pub enum DomainError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("invalid state: {0}")]
    InvalidState(String),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("storage error: {0}")]
    Storage(String),
}

mod collaboration;
pub use collaboration::*;
