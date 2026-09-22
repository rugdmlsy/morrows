use ac_core::{CreateArtifact, CreateDecision, CreateHandoff, CreateMessage, CreateThread};
use ac_core::{CreateContextRevision, CreateTask, Id, TaskState};
use ac_store::Store;
use axum::http::request::Parts;
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, tool::Extension, wrapper::Parameters},
    model::{ServerCapabilities, ServerConfig},
    schemars::JsonSchema,
    tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone)]
pub struct AgentCompanyMcp {
    pub store: Store,
    tool_router: ToolRouter<Self>,
}

impl AgentCompanyMcp {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            tool_router: Self::tool_router(),
        }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RegisterAgentRequest {
    pub name: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskIdRequest {
    pub task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateTaskRequest {
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub priority: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClaimTaskRequest {
    pub task_id: String,
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default = "default_lease")]
    pub lease_seconds: i64,
}
fn default_role() -> String {
    "executor".into()
}
fn default_lease() -> i64 {
    900
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct StartRunRequest {
    pub assignment_id: String,
    pub external_session_ref: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RenewAssignmentRequest {
    pub assignment_id: String,
    #[serde(default = "default_lease")]
    pub lease_seconds: i64,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckpointRunRequest {
    pub run_id: String,
    pub checkpoint: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompleteRunRequest {
    pub run_id: String,
    #[serde(default)]
    pub result: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReviseContextRequest {
    pub task_id: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub background: String,
    #[serde(default)]
    pub constraints: Value,
    #[serde(default)]
    pub current_summary: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateArtifactRequest {
    pub task_id: String,
    pub input: CreateArtifact,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateDecisionRequest {
    pub task_id: String,
    pub input: CreateDecision,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateThreadRequest {
    pub task_id: String,
    pub input: CreateThread,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateMessageRequest {
    pub thread_id: String,
    pub input: CreateMessage,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateHandoffRequest {
    pub run_id: String,
    pub input: CreateHandoff,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct AcceptHandoffRequest {
    pub handoff_id: String,
    pub target_run_id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct HandoffIdRequest {
    pub handoff_id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DependencyRequest {
    pub task_id: String,
    pub depends_on_task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FleetIdRequest {
    pub id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct InstanceIdRequest {
    pub agent_instance_id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct HeartbeatRequest {
    pub agent_instance_id: String,
    pub input: ac_core::AgentHeartbeat,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CapacityRequest {
    pub agent_instance_id: String,
    pub input: ac_core::RecordCapacity,
}

#[tool_router(router = tool_router)]
impl AgentCompanyMcp {
    #[tool(
        description = "Register or reuse a profile identity by its natural key. Existing rows are returned unchanged."
    )]
    async fn agent_profile_register(
        &self,
        Parameters(input): Parameters<ac_core::RegisterProfile>,
    ) -> Result<String, String> {
        let value = self
            .store
            .register_profile(input)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "List profile identities.")]
    async fn agent_profile_list(&self) -> Result<String, String> {
        let value = self
            .store
            .list_profiles()
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "Get a profile identity by UUID.")]
    async fn agent_profile_get(
        &self,
        Parameters(req): Parameters<FleetIdRequest>,
    ) -> Result<String, String> {
        let value = self
            .store
            .get_profile(parse_id(&req.id)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "Register or reuse a account identity by its natural key. Existing rows are returned unchanged."
    )]
    async fn account_register(
        &self,
        Parameters(input): Parameters<ac_core::RegisterAccount>,
    ) -> Result<String, String> {
        let value = self
            .store
            .register_account(input)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "List account identities.")]
    async fn account_list(&self) -> Result<String, String> {
        let value = self
            .store
            .list_accounts()
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "Get a account identity by UUID.")]
    async fn account_get(
        &self,
        Parameters(req): Parameters<FleetIdRequest>,
    ) -> Result<String, String> {
        let value = self
            .store
            .get_account(parse_id(&req.id)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "Register or reuse a machine identity by its natural key. Existing rows are returned unchanged."
    )]
    async fn machine_register(
        &self,
        Parameters(input): Parameters<ac_core::RegisterMachine>,
    ) -> Result<String, String> {
        let value = self
            .store
            .register_machine(input)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "List machine identities.")]
    async fn machine_list(&self) -> Result<String, String> {
        let value = self
            .store
            .list_machines()
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "Get a machine identity by UUID.")]
    async fn machine_get(
        &self,
        Parameters(req): Parameters<FleetIdRequest>,
    ) -> Result<String, String> {
        let value = self
            .store
            .get_machine(parse_id(&req.id)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "Register an instance with explicit profile and optional account/machine links. Omitted capabilities inherit profile defaults."
    )]
    async fn agent_instance_register(
        &self,
        Parameters(input): Parameters<ac_core::RegisterAgentInstance>,
    ) -> Result<String, String> {
        let value = self
            .store
            .register_agent_instance(input)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "Update own heartbeat/status and optionally append capacity atomically. Requires X-Agent-Instance-Id."
    )]
    async fn agent_heartbeat(
        &self,
        Parameters(req): Parameters<HeartbeatRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .agent_heartbeat(
                parse_id(&req.agent_instance_id)?,
                authenticated_agent(&parts)?,
                req.input,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "Append a capacity observation for own instance. Requires X-Agent-Instance-Id."
    )]
    async fn capacity_record(
        &self,
        Parameters(req): Parameters<CapacityRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .capacity_record(
                parse_id(&req.agent_instance_id)?,
                authenticated_agent(&parts)?,
                req.input,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "Read capacity latest for an instance. History is newest first; latest is null before any observation."
    )]
    async fn capacity_latest(
        &self,
        Parameters(req): Parameters<InstanceIdRequest>,
    ) -> Result<String, String> {
        let value = self
            .store
            .capacity_latest(parse_id(&req.agent_instance_id)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "Read capacity history for an instance. History is newest first; latest is null before any observation."
    )]
    async fn capacity_history(
        &self,
        Parameters(req): Parameters<InstanceIdRequest>,
    ) -> Result<String, String> {
        let value = self
            .store
            .capacity_history(parse_id(&req.agent_instance_id)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "List instances with their profile, account, machine and latest observed capacity."
    )]
    async fn agent_fleet(&self) -> Result<String, String> {
        let value = self.store.agent_fleet().await.map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Atomically accept a pending handoff with a live same-task target run owned by the caller. Requires X-Agent-Instance-Id."
    )]
    async fn handoff_accept(
        &self,
        Parameters(req): Parameters<AcceptHandoffRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .accept_handoff(
                parse_id(&req.handoff_id)?,
                parse_id(&req.target_run_id)?,
                authenticated_agent(&parts)?,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "Create a durable artifact. Requires X-Agent-Instance-Id.")]

    async fn artifact_create(
        &self,
        Parameters(req): Parameters<CreateArtifactRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .create_artifact(
                parse_id(&req.task_id)?,
                authenticated_agent(&parts)?,
                req.input,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "Create a durable decision. Requires X-Agent-Instance-Id.")]
    async fn decision_create(
        &self,
        Parameters(req): Parameters<CreateDecisionRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .create_decision(
                parse_id(&req.task_id)?,
                authenticated_agent(&parts)?,
                req.input,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "Create a durable thread. Requires X-Agent-Instance-Id.")]
    async fn thread_create(
        &self,
        Parameters(req): Parameters<CreateThreadRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .create_thread(
                parse_id(&req.task_id)?,
                authenticated_agent(&parts)?,
                req.input,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "Create a durable message. Requires X-Agent-Instance-Id.")]
    async fn message_create(
        &self,
        Parameters(req): Parameters<CreateMessageRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .create_message(
                parse_id(&req.thread_id)?,
                authenticated_agent(&parts)?,
                req.input,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "Create a durable handoff. Requires X-Agent-Instance-Id. Atomically ends source runs and releases assignment; requires task context."
    )]
    async fn handoff_create(
        &self,
        Parameters(req): Parameters<CreateHandoffRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .create_handoff(
                parse_id(&req.run_id)?,
                authenticated_agent(&parts)?,
                req.input,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "Read all handoffs, artifacts, decisions, message threads/messages and dependencies for a task."
    )]
    async fn task_collaboration(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
    ) -> Result<String, String> {
        let value = self
            .store
            .task_collaboration(parse_id(&req.task_id)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "Read a handoff with its pinned context revision and referenced artifacts/decisions for continuation without shared chat history."
    )]
    async fn handoff_get(
        &self,
        Parameters(req): Parameters<HandoffIdRequest>,
    ) -> Result<String, String> {
        let value = self
            .store
            .get_handoff(parse_id(&req.handoff_id)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "add a task prerequisite. Rejects cycles. Requires X-Agent-Instance-Id.")]
    async fn dependency_add(
        &self,
        Parameters(req): Parameters<DependencyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .add_dependency(
                parse_id(&req.task_id)?,
                parse_id(&req.depends_on_task_id)?,
                authenticated_agent(&parts)?,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(
        description = "remove a task prerequisite. Rejects cycles. Requires X-Agent-Instance-Id."
    )]
    async fn dependency_remove(
        &self,
        Parameters(req): Parameters<DependencyRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .remove_dependency(
                parse_id(&req.task_id)?,
                parse_id(&req.depends_on_task_id)?,
                authenticated_agent(&parts)?,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Register or refresh an agent instance. Returns its instance id. After registering, configure X-Agent-Instance-Id on subsequent MCP requests."
    )]
    async fn agent_register(
        &self,
        Parameters(req): Parameters<RegisterAgentRequest>,
    ) -> Result<String, String> {
        let agent = self
            .store
            .register_agent(&req.name, &req.capabilities)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&agent).map_err(|e| e.to_string())
    }

    #[tool(description = "List all tasks in the work queue.")]
    async fn task_list(&self) -> Result<String, String> {
        let tasks = self.store.list_tasks().await.map_err(|e| e.to_string())?;
        serde_json::to_string(&tasks).map_err(|e| e.to_string())
    }

    #[tool(description = "Get a task by id.")]
    async fn task_get(&self, Parameters(req): Parameters<TaskIdRequest>) -> Result<String, String> {
        let task = self
            .store
            .get_task(parse_id(&req.task_id)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&task).map_err(|e| e.to_string())
    }

    #[tool(description = "Create a new task. New tasks are ready by default.")]
    async fn task_create(
        &self,
        Parameters(req): Parameters<CreateTaskRequest>,
    ) -> Result<String, String> {
        let task = self
            .store
            .create_task(CreateTask {
                project_id: None,
                title: req.title,
                description: req.description,
                owner_actor_id: "human:local".into(),
                state: TaskState::Ready,
                priority: req.priority,
            })
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&task).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Claim a task role for the authenticated agent instance. Requires X-Agent-Instance-Id header."
    )]
    async fn task_claim(
        &self,
        Parameters(req): Parameters<ClaimTaskRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let assignment = self
            .store
            .claim_task(
                parse_id(&req.task_id)?,
                agent_id,
                &req.role,
                req.lease_seconds,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&assignment).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Renew an active assignment lease owned by the authenticated agent instance."
    )]
    async fn assignment_renew(
        &self,
        Parameters(req): Parameters<RenewAssignmentRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let assignment = self
            .store
            .renew_assignment(parse_id(&req.assignment_id)?, agent_id, req.lease_seconds)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&assignment).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Start a run for an assignment owned by the authenticated agent instance."
    )]
    async fn run_start(
        &self,
        Parameters(req): Parameters<StartRunRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let run = self
            .store
            .start_run(
                parse_id(&req.assignment_id)?,
                agent_id,
                req.external_session_ref,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&run).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Persist a durable checkpoint for a run owned by the authenticated agent instance."
    )]
    async fn run_checkpoint(
        &self,
        Parameters(req): Parameters<CheckpointRunRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let run = self
            .store
            .checkpoint_run(parse_id(&req.run_id)?, agent_id, req.checkpoint)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&run).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Complete a run owned by the authenticated agent instance. M1 marks the task done when its executor run completes."
    )]
    async fn run_complete(
        &self,
        Parameters(req): Parameters<CompleteRunRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let run = self
            .store
            .complete_run(parse_id(&req.run_id)?, agent_id, req.result)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&run).map_err(|e| e.to_string())
    }

    #[tool(description = "Get the current immutable context revision for a task.")]
    async fn context_get(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
    ) -> Result<String, String> {
        let context = self
            .store
            .get_current_context(parse_id(&req.task_id)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&context).map_err(|e| e.to_string())
    }

    #[tool(description = "Create a new immutable context revision for a task.")]
    async fn context_revise(
        &self,
        Parameters(req): Parameters<ReviseContextRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let actor = parts
            .headers
            .get("x-agent-instance-id")
            .and_then(|v| v.to_str().ok())
            .map(|v| format!("agent:{v}"))
            .unwrap_or_else(|| "human:local".into());
        let context = self
            .store
            .create_context_revision(
                parse_id(&req.task_id)?,
                CreateContextRevision {
                    goal: req.goal,
                    background: req.background,
                    constraints: if req.constraints.is_null() {
                        json!({})
                    } else {
                        req.constraints
                    },
                    current_summary: req.current_summary,
                    created_by_actor_id: actor,
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&context).map_err(|e| e.to_string())
    }

    #[tool(description = "List the append-only event timeline for a task.")]
    async fn task_events(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
    ) -> Result<String, String> {
        let events = self
            .store
            .task_events(parse_id(&req.task_id)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&events).map_err(|e| e.to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AgentCompanyMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Agent Company Work OS. Register an agent instance, then send X-Agent-Instance-Id on claim/run mutation requests. Tasks are independent of agent accounts and sessions."
            )
    }
}

fn parse_id(raw: &str) -> Result<Id, String> {
    Uuid::parse_str(raw).map_err(|_| format!("invalid UUID: {raw}"))
}

fn authenticated_agent(parts: &Parts) -> Result<Id, String> {
    let raw = parts
        .headers
        .get("x-agent-instance-id")
        .ok_or_else(|| "missing X-Agent-Instance-Id header".to_string())?
        .to_str()
        .map_err(|_| "invalid X-Agent-Instance-Id header".to_string())?;
    parse_id(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parts(actor: Option<Id>) -> Parts {
        let mut req = axum::http::Request::builder();
        if let Some(id) = actor {
            req = req.header("x-agent-instance-id", id.to_string());
        }
        req.body(()).unwrap().into_parts().0
    }
    #[tokio::test]
    async fn mcp_tools_expose_collaboration_and_enforce_header_identity() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = store.register_agent("mcp-a", &[]).await.unwrap();
        let b = store.register_agent("mcp-b", &[]).await.unwrap();
        let task = store
            .create_task(serde_json::from_value(json!({"title":"MCP"})).unwrap())
            .await
            .unwrap();
        store
            .create_context_revision(
                task.id,
                serde_json::from_value(json!({"goal":"MCP continuation"})).unwrap(),
            )
            .await
            .unwrap();
        let assignment = store
            .claim_task(task.id, a.id, "executor", 300)
            .await
            .unwrap();
        let run = store.start_run(assignment.id, a.id, None).await.unwrap();
        let mcp = AgentCompanyMcp::new(store.clone());
        for name in [
            "artifact_create",
            "decision_create",
            "thread_create",
            "message_create",
            "handoff_create",
            "handoff_get",
            "handoff_accept",
            "task_collaboration",
            "dependency_add",
            "dependency_remove",
        ] {
            assert!(mcp.tool_router.get(name).is_some(), "missing tool {name}");
        }
        let artifact_req = || {
            Parameters(CreateArtifactRequest {
                task_id: task.id.to_string(),
                input: CreateArtifact {
                    kind: "other".into(),
                    title: "patch".into(),
                    uri: "file:///patch".into(),
                    description: String::new(),
                },
            })
        };
        assert!(
            mcp.artifact_create(artifact_req(), Extension(parts(None)))
                .await
                .is_err()
        );
        let artifact: Value = serde_json::from_str(
            &mcp.artifact_create(artifact_req(), Extension(parts(Some(a.id))))
                .await
                .unwrap(),
        )
        .unwrap();
        let request = || {
            Parameters(CreateHandoffRequest {
                run_id: run.id.to_string(),
                input: CreateHandoff {
                    summary: "continue".into(),
                    completed: vec!["patch".into()],
                    remaining: vec!["test".into()],
                    blockers: vec![],
                    artifact_ids: vec![artifact["id"].as_str().unwrap().into()],
                    decision_ids: vec![],
                },
            })
        };
        assert!(
            mcp.handoff_create(request(), Extension(parts(Some(b.id))))
                .await
                .is_err()
        );
        let h: Value = serde_json::from_str(
            &mcp.handoff_create(request(), Extension(parts(Some(a.id))))
                .await
                .unwrap(),
        )
        .unwrap();
        let recovered: Value = serde_json::from_str(
            &mcp.handoff_get(Parameters(HandoffIdRequest {
                handoff_id: h["id"].as_str().unwrap().into(),
            }))
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(recovered["context"]["goal"], "MCP continuation");
        let next = store
            .claim_task(task.id, b.id, "executor", 300)
            .await
            .unwrap();
        let target = store.start_run(next.id, b.id, None).await.unwrap();
        let accept = || {
            Parameters(AcceptHandoffRequest {
                handoff_id: h["id"].as_str().unwrap().into(),
                target_run_id: target.id.to_string(),
            })
        };
        assert!(
            mcp.handoff_accept(accept(), Extension(parts(None)))
                .await
                .is_err()
        );
        assert!(
            mcp.handoff_accept(accept(), Extension(parts(Some(a.id))))
                .await
                .is_err()
        );
        let accepted: Value = serde_json::from_str(
            &mcp.handoff_accept(accept(), Extension(parts(Some(b.id))))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(accepted["status"], "accepted");
        assert_eq!(accepted["accepted_by_run_id"], json!(target.id));
        assert!(
            mcp.handoff_accept(accept(), Extension(parts(Some(b.id))))
                .await
                .is_err()
        );

        assert_eq!(recovered["artifacts"][0]["id"], artifact["id"]);
        assert!(
            mcp.task_collaboration(Parameters(TaskIdRequest {
                task_id: "bad-id".into()
            }))
            .await
            .is_err()
        );
    }
}

#[cfg(test)]
#[path = "mcp_fleet_tests.rs"]
mod fleet_tests;
