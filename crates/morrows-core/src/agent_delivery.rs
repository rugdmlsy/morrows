use crate::Id;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDelivery {
    pub id: Id,
    pub agent_instance_id: Id,
    pub task_id: Option<Id>,
    pub kind: String,
    pub source_id: Id,
    pub payload: Value,
    pub status: String,
    pub claimed_by_launch_attempt_id: Option<Id>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub delivered_by: Option<String>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}
