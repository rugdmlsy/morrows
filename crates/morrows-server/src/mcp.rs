use axum::http::request::Parts;
use morrows_core::{
    ConversationHistoryRequest, ConversationReply, CreateArtifact, CreateDecision, CreateHandoff,
    CreateMessage, CreateThread,
};
use morrows_core::{CreateContextRevision, CreateTask, Id, TaskState};
use morrows_store::Store;
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
pub struct MorrowsMcp {
    pub store: Store,
    tool_router: ToolRouter<Self>,
}

impl MorrowsMcp {
    pub fn new(store: Store) -> Self {
        Self {
            store,
            tool_router: Self::tool_router(),
        }
    }

    async fn ensure_task_access(&self, task_id: Id, agent_id: Id) -> Result<(), String> {
        self.store
            .get_agent(agent_id)
            .await
            .map_err(|e| e.to_string())?;
        let task = self
            .store
            .get_task(task_id)
            .await
            .map_err(|e| e.to_string())?;
        if task.owner_actor_id == format!("agent:{agent_id}") {
            return Ok(());
        }
        let assignments = self
            .store
            .task_assignments(task_id)
            .await
            .map_err(|e| e.to_string())?;
        if assignments
            .iter()
            .any(|assignment| assignment.agent_instance_id == agent_id)
        {
            return Ok(());
        }
        Err("task is not owned by or assigned to the authenticated agent instance".into())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskIdRequest {
    pub task_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorkRequestSubmitRequest {
    pub title: String,
    #[serde(default)]
    pub description: String,
}

fn default_lease() -> i64 {
    900
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
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub checkpoint: Value,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompleteRunRequest {
    pub run_id: String,
    #[serde(default)]
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
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
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
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
    pub task_id: String,
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
#[tool_router(router = tool_router)]
impl MorrowsMcp {
    #[tool(
        description = "List direct company conversations with queued human messages addressed to the authenticated employee. Returns summaries only; use conversation_get to load one history. Requires X-Agent-Instance-Id."
    )]
    async fn conversation_inbox(
        &self,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .agent_conversation_inbox(authenticated_agent(&parts)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read one direct company conversation addressed to the authenticated employee. History is paged; before_message_id loads older messages and after_message_id loads newer messages. Requires X-Agent-Instance-Id."
    )]
    async fn conversation_get(
        &self,
        Parameters(req): Parameters<ConversationHistoryRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let before = req.before_message_id.as_deref().map(parse_id).transpose()?;
        let after = req.after_message_id.as_deref().map(parse_id).transpose()?;
        let value = self
            .store
            .agent_conversation_history(
                parse_id(&req.conversation_id)?,
                authenticated_agent(&parts)?,
                before,
                after,
                req.limit.unwrap_or(80),
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Reply to a direct company conversation addressed to the authenticated employee. The reply marks queued human messages in that conversation delivered. Requires X-Agent-Instance-Id."
    )]
    async fn conversation_reply(
        &self,
        Parameters(req): Parameters<ConversationReply>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .agent_reply_conversation(
                parse_id(&req.conversation_id)?,
                authenticated_agent(&parts)?,
                &req.body,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read management instructions for a work item owned by or assigned to the authenticated employee. Requires X-Agent-Instance-Id."
    )]
    async fn instructions_get(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let value = self
            .store
            .task_launch_instructions(task_id)
            .await
            .map_err(|e| e.to_string())?;
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
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let value = self
            .store
            .create_artifact(task_id, agent_id, req.input)
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
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let value = self
            .store
            .create_decision(task_id, agent_id, req.input)
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
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let value = self
            .store
            .create_thread(task_id, agent_id, req.input)
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
        let task_id = parse_id(&req.task_id)?;
        let thread_id = parse_id(&req.thread_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let threads = self
            .store
            .task_threads(task_id)
            .await
            .map_err(|e| e.to_string())?;
        if !threads.iter().any(|thread| thread.id == thread_id) {
            return Err("thread does not belong to the supplied task".into());
        }
        let value = self
            .store
            .create_message(thread_id, agent_id, req.input)
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
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let value = self
            .store
            .task_collaboration(task_id)
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
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let value = self
            .store
            .get_handoff(parse_id(&req.handoff_id)?)
            .await
            .map_err(|e| e.to_string())?;
        self.ensure_task_access(value.handoff.task_id, agent_id)
            .await?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read a work item owned by or assigned to the authenticated employee. Requires X-Agent-Instance-Id."
    )]
    async fn task_get(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let task = self
            .store
            .get_task(task_id)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&task).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Submit a work request to Morrows for company scheduling. The caller becomes the request owner; employees cannot set dispatch priority or claim the work themselves. Requires X-Agent-Instance-Id."
    )]
    async fn work_request_submit(
        &self,
        Parameters(req): Parameters<WorkRequestSubmitRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        self.store
            .get_agent(agent_id)
            .await
            .map_err(|e| e.to_string())?;
        let task = self
            .store
            .create_task(CreateTask {
                project_id: None,
                title: req.title,
                description: req.description,
                owner_actor_id: format!("agent:{agent_id}"),
                state: TaskState::Ready,
                priority: 0,
            })
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&task).map_err(|e| e.to_string())
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

    #[tool(
        description = "Pull the current durable working memory for a work item owned by or assigned to the authenticated employee. Requires X-Agent-Instance-Id."
    )]
    async fn memory_get(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let task = self
            .store
            .get_task(task_id)
            .await
            .map_err(|e| e.to_string())?;
        let context = match task.current_context_revision_id {
            Some(_) => Some(
                self.store
                    .get_current_context(task_id)
                    .await
                    .map_err(|e| e.to_string())?,
            ),
            None => None,
        };
        let collaboration = self
            .store
            .task_collaboration(task_id)
            .await
            .map_err(|e| e.to_string())?;
        let my_runs = self
            .store
            .task_runs(task_id)
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|run| run.agent_instance_id == agent_id)
            .collect::<Vec<_>>();
        serde_json::to_string(&json!({
            "task": task,
            "context": context,
            "collaboration": collaboration,
            "my_runs": my_runs,
        }))
        .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Write a new immutable working-memory revision for a work item owned by or assigned to the authenticated employee. Requires X-Agent-Instance-Id."
    )]
    async fn memory_revise(
        &self,
        Parameters(req): Parameters<ReviseContextRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let context = self
            .store
            .create_context_revision(
                task_id,
                CreateContextRevision {
                    goal: req.goal,
                    background: req.background,
                    constraints: if req.constraints.is_null() {
                        json!({})
                    } else {
                        req.constraints
                    },
                    current_summary: req.current_summary,
                    created_by_actor_id: format!("agent:{agent_id}"),
                },
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&context).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read the append-only event timeline for a work item owned by or assigned to the authenticated employee. Requires X-Agent-Instance-Id."
    )]
    async fn task_events(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let events = self
            .store
            .task_events(task_id)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&events).map_err(|e| e.to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MorrowsMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Morrows employee interface. The company control plane owns registration, fleet state, dispatch, assignment, Run creation, launch, cancellation, and scheduling. Employees may read/reply to direct company conversations, access only work they own or have been assigned, pull/update working memory, receive instructions, collaborate, hand off work, renew an existing lease, and report progress or completion. Send X-Agent-Instance-Id on every MCP tool call."
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

    #[test]
    fn arbitrary_json_mcp_fields_use_object_property_schemas() {
        for (schema, field) in [
            (
                serde_json::to_value(rmcp::schemars::schema_for!(CheckpointRunRequest)).unwrap(),
                "checkpoint",
            ),
            (
                serde_json::to_value(rmcp::schemars::schema_for!(CompleteRunRequest)).unwrap(),
                "result",
            ),
            (
                serde_json::to_value(rmcp::schemars::schema_for!(ReviseContextRequest)).unwrap(),
                "constraints",
            ),
        ] {
            assert!(
                schema["properties"][field].is_object(),
                "{field} must be an object JSON Schema for strict MCP clients"
            );
        }
    }

    #[tokio::test]
    async fn mcp_surface_is_employee_only() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let mcp = MorrowsMcp::new(store);
        for name in [
            "conversation_inbox",
            "conversation_get",
            "conversation_reply",
            "work_request_submit",
            "task_get",
            "memory_get",
            "memory_revise",
            "instructions_get",
            "artifact_create",
            "decision_create",
            "thread_create",
            "message_create",
            "handoff_create",
            "handoff_get",
            "handoff_accept",
            "task_collaboration",
            "assignment_renew",
            "run_checkpoint",
            "run_complete",
            "task_events",
        ] {
            assert!(
                mcp.tool_router.get(name).is_some(),
                "missing employee tool {name}"
            );
        }
        for name in [
            "agent_profile_register",
            "agent_profile_list",
            "account_register",
            "machine_register",
            "agent_instance_register",
            "agent_register",
            "agent_heartbeat",
            "capacity_record",
            "agent_fleet",
            "dispatch_policy_set",
            "dispatch_preview",
            "dispatch_task",
            "dispatch_next",
            "launch_profile_register",
            "launch_enqueue",
            "launch_stop",
            "external_launch_list",
            "external_launch_accept",
            "launch_instruction_send",
            "task_list",
            "task_create",
            "task_claim",
            "run_start",
            "dependency_add",
            "dependency_remove",
        ] {
            assert!(
                mcp.tool_router.get(name).is_none(),
                "control-plane tool leaked into employee MCP: {name}"
            );
        }
    }

    #[tokio::test]
    async fn direct_conversations_are_scoped_to_the_addressed_employee() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = store.register_agent("chat-a", &[]).await.unwrap();
        let b = store.register_agent("chat-b", &[]).await.unwrap();
        let conversation = store
            .create_conversation(morrows_core::CreateConversation {
                agent_instance_id: a.id,
                title: "Direct".into(),
            })
            .await
            .unwrap();
        store
            .create_human_conversation_message(conversation.id, "hello")
            .await
            .unwrap();
        let mcp = MorrowsMcp::new(store.clone());

        let inbox: Value = serde_json::from_str(
            &mcp.conversation_inbox(Extension(parts(Some(a.id))))
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(inbox.as_array().unwrap().len(), 1);
        assert!(
            mcp.conversation_get(
                Parameters(ConversationHistoryRequest {
                    conversation_id: conversation.id.to_string(),
                    before_message_id: None,
                    after_message_id: None,
                    limit: Some(20),
                }),
                Extension(parts(Some(b.id))),
            )
            .await
            .is_err()
        );
        mcp.conversation_reply(
            Parameters(ConversationReply {
                conversation_id: conversation.id.to_string(),
                body: "hi".into(),
            }),
            Extension(parts(Some(a.id))),
        )
        .await
        .unwrap();
        assert!(
            store
                .agent_conversation_inbox(a.id)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn work_request_submit_uses_employee_identity_without_priority_control() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let employee = store.register_agent("employee", &[]).await.unwrap();
        let mcp = MorrowsMcp::new(store.clone());
        assert!(
            mcp.work_request_submit(
                Parameters(WorkRequestSubmitRequest {
                    title: "Need review".into(),
                    description: "Please review this change".into(),
                }),
                Extension(parts(None)),
            )
            .await
            .is_err()
        );
        let task: Value = serde_json::from_str(
            &mcp.work_request_submit(
                Parameters(WorkRequestSubmitRequest {
                    title: "Need review".into(),
                    description: "Please review this change".into(),
                }),
                Extension(parts(Some(employee.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(task["owner_actor_id"], format!("agent:{}", employee.id));
        assert_eq!(task["priority"], 0);
        let events = store
            .task_events(parse_id(task["id"].as_str().unwrap()).unwrap())
            .await
            .unwrap();
        assert_eq!(events[0].actor_type, "agent_instance");
        assert_eq!(events[0].actor_id, employee.id.to_string());
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
        let mcp = MorrowsMcp::new(store.clone());
        for name in [
            "artifact_create",
            "decision_create",
            "thread_create",
            "message_create",
            "handoff_create",
            "handoff_get",
            "handoff_accept",
            "task_collaboration",
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
            &mcp.handoff_get(
                Parameters(HandoffIdRequest {
                    handoff_id: h["id"].as_str().unwrap().into(),
                }),
                Extension(parts(Some(a.id))),
            )
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
            mcp.task_collaboration(
                Parameters(TaskIdRequest {
                    task_id: "bad-id".into()
                }),
                Extension(parts(Some(a.id))),
            )
            .await
            .is_err()
        );
    }
}
