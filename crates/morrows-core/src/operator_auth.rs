use crate::Id;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorCredential {
    pub id: Id,
    pub label: String,
    pub role: String,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssuedOperatorCredential {
    pub credential: OperatorCredential,
    /// Returned only at issuance. Morrows stores only the SHA-256 hash.
    pub token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperatorLoginRequest {
    pub id: Id,
    pub code: String,
    /// Kept in the requesting browser; never printed in the approval command.
    pub token: String,
    pub expires_at: DateTime<Utc>,
}
