use crate::Id;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssignmentRequest {
    pub id: Id,
    pub task_id: Id,
    pub agent_instance_id: Id,
    pub role: String,
    pub reason: String,
    pub status: String,
    pub assignment_id: Option<Id>,
    pub resolution: Option<String>,
    pub created_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
}
