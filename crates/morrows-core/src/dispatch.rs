use super::*;
use schemars::JsonSchema;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TaskContinuationPolicy {
    pub enabled: bool,
    /// Ordered, authorized candidates. Each is automatically assigned at most once per task.
    #[schemars(with = "Vec<String>")]
    pub agent_ids: Vec<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SetDispatchPolicy {
    #[serde(default = "executor_role")]
    pub role: String,
    #[serde(default)]
    pub required_capabilities: Vec<String>,
    #[schemars(with = "Option<String>")]
    pub profile_id: Option<Id>,
    #[schemars(with = "Option<String>")]
    pub account_id: Option<Id>,
    #[schemars(with = "Option<String>")]
    pub machine_id: Option<Id>,
    #[serde(default = "default_ttl")]
    pub heartbeat_ttl_seconds: i64,
    #[serde(default = "default_ttl")]
    pub capacity_ttl_seconds: i64,
    #[serde(default = "default_lease")]
    pub lease_seconds: i64,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDispatchPolicy {
    pub task_id: Id,
    pub role: String,
    pub required_capabilities: Vec<String>,
    pub profile_id: Option<Id>,
    pub account_id: Option<Id>,
    pub machine_id: Option<Id>,
    pub heartbeat_ttl_seconds: i64,
    pub capacity_ttl_seconds: i64,
    pub lease_seconds: i64,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchCandidate {
    pub agent_instance_id: Id,
    pub agent_name: String,
    pub eligible: bool,
    pub effective_slots: i64,
    pub current_active_assignments: i64,
    pub heartbeat_age_seconds: i64,
    pub capacity_age_seconds: Option<i64>,
    pub capacity_status: Option<String>,
    pub quota_state: Option<String>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchPreview {
    pub task_id: Id,
    pub role: String,
    pub policy: TaskDispatchPolicy,
    pub task_dispatchable: bool,
    pub task_reasons: Vec<String>,
    pub candidates: Vec<DispatchCandidate>,
    pub selected_agent_instance_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchDecision {
    pub id: Id,
    pub task_id: Id,
    pub role: String,
    pub outcome: String,
    pub selected_agent_instance_id: Option<Id>,
    pub assignment_id: Option<Id>,
    pub preview: DispatchPreview,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchOutcome {
    pub decision: DispatchDecision,
    pub assignment: Option<Assignment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchNextResult {
    pub dispatched: Option<DispatchOutcome>,
    pub attempts: Vec<DispatchOutcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchSchedulerSettings {
    pub role: String,
    pub enabled: bool,
    pub interval_seconds: i64,
    pub auto_launch: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SetDispatchSchedulerSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_scheduler_interval")]
    pub interval_seconds: i64,
    #[serde(default = "default_enabled")]
    pub auto_launch: bool,
}

fn executor_role() -> String {
    "executor".into()
}
fn default_ttl() -> i64 {
    120
}
fn default_lease() -> i64 {
    900
}
fn default_enabled() -> bool {
    true
}
fn default_scheduler_interval() -> i64 {
    2
}
