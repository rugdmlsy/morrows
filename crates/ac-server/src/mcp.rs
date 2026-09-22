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
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Clone)]
pub struct AgentCompanyMcp {
    pub store: Store,
    tool_router: ToolRouter<Self>,
}

impl AgentCompanyMcp {
    pub fn new(store: Store) -> Self {
        Self { store, tool_router: Self::tool_router() }
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RegisterAgentRequest {
    pub name: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskIdRequest { pub task_id: String }

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
fn default_role() -> String { "executor".into() }
fn default_lease() -> i64 { 900 }

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

#[tool_router(router = tool_router)]
impl AgentCompanyMcp {
    #[tool(description = "Register or refresh an agent instance. Returns its instance id. After registering, configure X-Agent-Instance-Id on subsequent MCP requests.")]
    async fn agent_register(&self, Parameters(req): Parameters<RegisterAgentRequest>) -> Result<String, String> {
        let agent = self.store.register_agent(&req.name, &req.capabilities).await.map_err(|e| e.to_string())?;
        serde_json::to_string(&agent).map_err(|e| e.to_string())
    }

    #[tool(description = "List all tasks in the work queue.")]
    async fn task_list(&self) -> Result<String, String> {
        let tasks = self.store.list_tasks().await.map_err(|e| e.to_string())?;
        serde_json::to_string(&tasks).map_err(|e| e.to_string())
    }

    #[tool(description = "Get a task by id.")]
    async fn task_get(&self, Parameters(req): Parameters<TaskIdRequest>) -> Result<String, String> {
        let task = self.store.get_task(parse_id(&req.task_id)?).await.map_err(|e| e.to_string())?;
        serde_json::to_string(&task).map_err(|e| e.to_string())
    }

    #[tool(description = "Create a new task. New tasks are ready by default.")]
    async fn task_create(&self, Parameters(req): Parameters<CreateTaskRequest>) -> Result<String, String> {
        let task = self.store.create_task(CreateTask {
            project_id: None,
            title: req.title,
            description: req.description,
            owner_actor_id: "human:local".into(),
            state: TaskState::Ready,
            priority: req.priority,
        }).await.map_err(|e| e.to_string())?;
        serde_json::to_string(&task).map_err(|e| e.to_string())
    }

    #[tool(description = "Claim a task role for the authenticated agent instance. Requires X-Agent-Instance-Id header.")]
    async fn task_claim(
        &self,
        Parameters(req): Parameters<ClaimTaskRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let assignment = self.store.claim_task(parse_id(&req.task_id)?, agent_id, &req.role, req.lease_seconds)
            .await.map_err(|e| e.to_string())?;
        serde_json::to_string(&assignment).map_err(|e| e.to_string())
    }

    #[tool(description = "Renew an active assignment lease owned by the authenticated agent instance.")]
    async fn assignment_renew(
        &self,
        Parameters(req): Parameters<RenewAssignmentRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let assignment = self.store.renew_assignment(parse_id(&req.assignment_id)?, agent_id, req.lease_seconds)
            .await.map_err(|e| e.to_string())?;
        serde_json::to_string(&assignment).map_err(|e| e.to_string())
    }

    #[tool(description = "Start a run for an assignment owned by the authenticated agent instance.")]
    async fn run_start(
        &self,
        Parameters(req): Parameters<StartRunRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let run = self.store.start_run(parse_id(&req.assignment_id)?, agent_id, req.external_session_ref)
            .await.map_err(|e| e.to_string())?;
        serde_json::to_string(&run).map_err(|e| e.to_string())
    }

    #[tool(description = "Persist a durable checkpoint for a run owned by the authenticated agent instance.")]
    async fn run_checkpoint(
        &self,
        Parameters(req): Parameters<CheckpointRunRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let run = self.store.checkpoint_run(parse_id(&req.run_id)?, agent_id, req.checkpoint)
            .await.map_err(|e| e.to_string())?;
        serde_json::to_string(&run).map_err(|e| e.to_string())
    }

    #[tool(description = "Complete a run owned by the authenticated agent instance. M1 marks the task done when its executor run completes.")]
    async fn run_complete(
        &self,
        Parameters(req): Parameters<CompleteRunRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let run = self.store.complete_run(parse_id(&req.run_id)?, agent_id, req.result)
            .await.map_err(|e| e.to_string())?;
        serde_json::to_string(&run).map_err(|e| e.to_string())
    }

    #[tool(description = "Get the current immutable context revision for a task.")]
    async fn context_get(&self, Parameters(req): Parameters<TaskIdRequest>) -> Result<String, String> {
        let context = self.store.get_current_context(parse_id(&req.task_id)?).await.map_err(|e| e.to_string())?;
        serde_json::to_string(&context).map_err(|e| e.to_string())
    }

    #[tool(description = "Create a new immutable context revision for a task.")]
    async fn context_revise(
        &self,
        Parameters(req): Parameters<ReviseContextRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let actor = parts.headers.get("x-agent-instance-id")
            .and_then(|v| v.to_str().ok())
            .map(|v| format!("agent:{v}"))
            .unwrap_or_else(|| "human:local".into());
        let context = self.store.create_context_revision(parse_id(&req.task_id)?, CreateContextRevision {
            goal: req.goal,
            background: req.background,
            constraints: if req.constraints.is_null() { json!({}) } else { req.constraints },
            current_summary: req.current_summary,
            created_by_actor_id: actor,
        }).await.map_err(|e| e.to_string())?;
        serde_json::to_string(&context).map_err(|e| e.to_string())
    }

    #[tool(description = "List the append-only event timeline for a task.")]
    async fn task_events(&self, Parameters(req): Parameters<TaskIdRequest>) -> Result<String, String> {
        let events = self.store.task_events(parse_id(&req.task_id)?).await.map_err(|e| e.to_string())?;
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
    let raw = parts.headers.get("x-agent-instance-id")
        .ok_or_else(|| "missing X-Agent-Instance-Id header".to_string())?
        .to_str().map_err(|_| "invalid X-Agent-Instance-Id header".to_string())?;
    parse_id(raw)
}
