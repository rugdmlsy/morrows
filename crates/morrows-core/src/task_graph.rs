use super::*;

/// Conditions inspect only the completed executor Run's structured result.
/// `path` is an RFC 6901 JSON Pointer; a missing path never matches, even null.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EdgeCondition {
    #[default]
    Unconditional,
    ResultEquals {
        path: String,
        value: Value,
    },
}
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JoinMode {
    #[default]
    All,
    Any,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GateState {
    Eligible,
    Waiting,
    Excluded,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskGate {
    pub task_id: Id,
    pub join_mode: JoinMode,
    pub state: GateState,
    pub predecessors: Vec<EdgeGate>,
    pub successors: Vec<Id>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeGate {
    pub predecessor_id: Id,
    pub condition: EdgeCondition,
    pub state: GateState,
    pub reason: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCollection {
    pub id: Id,
    pub name: String,
    pub description: String,
    pub archived: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
pub type TaskChain = TaskCollection;
pub type TaskGroup = TaskCollection;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveTaskCollection {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub archived: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveChainEdge {
    pub task_id: Id,
    pub depends_on_task_id: Id,
    #[serde(default)]
    pub condition: EdgeCondition,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupBatchAssign {
    pub group_id: Id,
    pub agent_instance_id: Id,
    #[serde(default = "batch_lease")]
    pub lease_seconds: i64,
}
fn batch_lease() -> i64 {
    3600
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchAssignmentResult {
    pub task_id: Id,
    pub status: String,
    pub assignment: Option<Assignment>,
    pub gate: TaskGate,
    pub error: Option<String>,
}
