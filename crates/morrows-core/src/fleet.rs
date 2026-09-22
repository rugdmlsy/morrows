use super::*;
use schemars::JsonSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: Id,
    pub name: String,
    pub provider: String,
    pub kind: String,
    pub default_capabilities: Vec<String>,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RegisterProfile {
    pub name: String,
    pub provider: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub default_capabilities: Vec<String>,
    #[serde(default = "empty_object")]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: Id,
    pub provider: String,
    pub label: String,
    pub external_account_ref: Option<String>,
    pub status: String,
    pub metadata: Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RegisterAccount {
    pub provider: String,
    pub label: String,
    pub external_account_ref: Option<String>,
    #[serde(default = "active_status")]
    pub status: String,
    #[serde(default = "empty_object")]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Machine {
    pub id: Id,
    pub name: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub status: String,
    pub metadata: Value,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RegisterMachine {
    pub name: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(default)]
    pub os: String,
    #[serde(default)]
    pub arch: String,
    #[serde(default = "active_status")]
    pub status: String,
    #[serde(default = "empty_object")]
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RegisterAgentInstance {
    #[schemars(with = "String")]
    pub profile_id: Id,
    #[schemars(with = "Option<String>")]
    pub account_id: Option<Id>,
    #[schemars(with = "Option<String>")]
    pub machine_id: Option<Id>,
    pub name: String,
    /// Omitted capabilities inherit the profile defaults; an explicit [] stays empty.
    pub capabilities: Option<Vec<String>>,
    pub external_instance_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RecordCapacity {
    pub status: String,
    pub available_slots: i64,
    pub active_assignments: i64,
    pub active_runs: i64,
    pub max_concurrency: Option<i64>,
    pub quota_state: Option<String>,
    #[serde(default = "empty_object")]
    pub details: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapacitySnapshot {
    pub id: Id,
    pub agent_instance_id: Id,
    #[serde(flatten)]
    pub capacity: RecordCapacity,
    pub observed_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AgentHeartbeat {
    pub status: String,
    pub capacity: Option<RecordCapacity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetEntry {
    pub instance: AgentInstance,
    pub profile: AgentProfile,
    pub account: Option<Account>,
    pub machine: Option<Machine>,
    pub latest_capacity: Option<CapacitySnapshot>,
}

fn active_status() -> String {
    "active".into()
}
