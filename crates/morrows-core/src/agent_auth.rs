use crate::Id;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentCredential {
    pub id: Id,
    pub agent_instance_id: Id,
    pub kind: String,
    pub label: String,
    pub run_id: Option<Id>,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuedAgentCredential {
    pub credential: AgentCredential,
    /// Returned only by the issuance operation. Morrows persists only its SHA-256 hash.
    pub token: String,
}
