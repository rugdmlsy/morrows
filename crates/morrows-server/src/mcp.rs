use axum::http::request::Parts;
use morrows_core::{
    ConversationHistoryRequest, ConversationReply, CreateArtifact,
    CreateConversationSummaryRevision, CreateDecision, CreateHandoff, CreateMessage, CreateThread,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ConversationSummaryRequest {
    pub conversation_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeliveryInboxRequest {
    pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeliveryAckRequest {
    pub delivery_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReviseConversationSummaryRequest {
    pub conversation_id: String,
    #[serde(default)]
    pub goal: String,
    #[serde(default)]
    pub current_state: String,
    #[serde(default)]
    pub important_findings: Vec<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub blockers: Vec<String>,
    #[serde(default)]
    pub unresolved_questions: Vec<String>,
    #[serde(default)]
    pub next_steps: Vec<String>,
    #[serde(default)]
    pub deterministic_facts: std::collections::BTreeMap<String, String>,
}

#[tool_router(router = tool_router)]
impl MorrowsMcp {
    #[tool(
        description = "List direct company conversations with queued human messages addressed to the authenticated employee. Returns summaries only; use conversation_get to load one history. Requires authenticated Agent identity."
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
        description = "Read one direct company conversation addressed to the authenticated employee. History is paged; before_message_id loads older messages and after_message_id loads newer messages. Requires authenticated Agent identity."
    )]
    async fn conversation_get(
        &self,
        Parameters(req): Parameters<ConversationHistoryRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let before = req.before_message_id.as_deref().map(parse_id).transpose()?;
        let after = req.after_message_id.as_deref().map(parse_id).transpose()?;
        let conversation_id = parse_id(&req.conversation_id)?;
        let agent_id = authenticated_agent(&parts)?;
        let value = self
            .store
            .agent_conversation_history(
                conversation_id,
                agent_id,
                before,
                after,
                req.limit.unwrap_or(80),
            )
            .await
            .map_err(|e| e.to_string())?;
        self.store
            .acknowledge_conversation_deliveries(
                conversation_id,
                agent_id,
                &format!("conversation_get:{agent_id}"),
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Reply to a direct company conversation addressed to the authenticated employee. The reply marks queued human messages in that conversation delivered. Requires authenticated Agent identity."
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
        description = "Read the latest structured summary revision for a direct company conversation addressed to the authenticated employee. Requires authenticated Agent identity."
    )]
    async fn conversation_summary_get(
        &self,
        Parameters(req): Parameters<ConversationSummaryRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let conversation_id = parse_id(&req.conversation_id)?;
        let agent_id = authenticated_agent(&parts)?;
        let conversation = self
            .store
            .get_conversation(conversation_id)
            .await
            .map_err(|e| e.to_string())?;
        if conversation.agent_instance_id != agent_id {
            return Err("conversation belongs to another agent instance".into());
        }
        let summary = self
            .store
            .get_latest_conversation_summary_revision(conversation_id)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&summary).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Create a new structured summary revision for a direct company conversation addressed to the authenticated employee. Morrows automatically links the previous summary revision and covers the latest message currently in the conversation. Requires authenticated Agent identity."
    )]
    async fn conversation_summary_revise(
        &self,
        Parameters(req): Parameters<ReviseConversationSummaryRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let conversation_id = parse_id(&req.conversation_id)?;
        let agent_id = authenticated_agent(&parts)?;
        let conversation = self
            .store
            .get_conversation(conversation_id)
            .await
            .map_err(|e| e.to_string())?;
        if conversation.agent_instance_id != agent_id {
            return Err("conversation belongs to another agent instance".into());
        }

        let previous_revision_id = self
            .store
            .get_latest_conversation_summary_revision(conversation_id)
            .await
            .map_err(|e| e.to_string())?
            .map(|summary| summary.id);
        let history = self
            .store
            .conversation_history(conversation_id, None, None, 1)
            .await
            .map_err(|e| e.to_string())?;
        let covers_until_message_id = history.messages.last().map(|message| message.id);

        let summary = self
            .store
            .create_conversation_summary_revision(CreateConversationSummaryRevision {
                conversation_id,
                previous_revision_id,
                covers_until_message_id,
                goal: req.goal,
                current_state: req.current_state,
                important_findings: json!(req.important_findings),
                decisions: json!(req.decisions),
                blockers: json!(req.blockers),
                unresolved_questions: json!(req.unresolved_questions),
                next_steps: json!(req.next_steps),
                deterministic_facts: json!(req.deterministic_facts),
                created_by: format!("agent:{agent_id}"),
            })
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&summary).map_err(|e| e.to_string())
    }

    #[tool(
        description = "List durable Morrows deliveries queued for the authenticated employee. A delivery references an existing conversation message or launch instruction; process the referenced source and then call delivery_ack. Requires authenticated Agent identity."
    )]
    async fn delivery_inbox(
        &self,
        Parameters(req): Parameters<DeliveryInboxRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let value = self
            .store
            .agent_delivery_inbox(agent_id, req.limit.unwrap_or(80))
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Acknowledge one durable Morrows delivery after the authenticated employee or its provider bridge has received it. A delivery can only be acknowledged by its target AgentInstance. Requires authenticated Agent identity."
    )]
    async fn delivery_ack(
        &self,
        Parameters(req): Parameters<DeliveryAckRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let value = self
            .store
            .acknowledge_agent_delivery(
                parse_id(&req.delivery_id)?,
                agent_id,
                &format!("employee_mcp:{agent_id}"),
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read management instructions for a work item owned by or assigned to the authenticated employee. Requires authenticated Agent identity."
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
        self.store
            .acknowledge_instruction_deliveries_for_task(
                task_id,
                agent_id,
                &format!("instructions_get:{agent_id}"),
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Atomically accept a pending handoff with a live same-task target run owned by the caller. Requires authenticated Agent identity."
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
    #[tool(description = "Create a durable artifact. Requires authenticated Agent identity.")]

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
    #[tool(description = "Create a durable decision. Requires authenticated Agent identity.")]
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
    #[tool(description = "Create a durable thread. Requires authenticated Agent identity.")]
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
    #[tool(description = "Create a durable message. Requires authenticated Agent identity.")]
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
        description = "Create a durable handoff. Requires authenticated Agent identity. Atomically ends source runs and releases assignment; requires task context."
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
        description = "Read a work item owned by or assigned to the authenticated employee. Requires authenticated Agent identity."
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
        description = "Submit a work request to Morrows for company scheduling. The caller becomes the request owner; employees cannot set dispatch priority or claim the work themselves. Requires authenticated Agent identity."
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
        description = "Pull the current durable working memory for a work item owned by or assigned to the authenticated employee. Requires authenticated Agent identity."
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
        description = "Write a new immutable working-memory revision for a work item owned by or assigned to the authenticated employee. Requires authenticated Agent identity."
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
        description = "Read the append-only event timeline for a work item owned by or assigned to the authenticated employee. Requires authenticated Agent identity."
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

    #[tool(
        description = "Read the assembled ContextPackage for a work item owned by or assigned to the authenticated employee, containing objective, pinned context snapshot, memory refs, decisions, artifacts, verification status, and next action. If not yet created, automatically assembles one from current durable state. Requires authenticated Agent identity."
    )]
    async fn context_package_get(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let package = match self
            .store
            .get_latest_context_package(task_id)
            .await
            .map_err(|e| e.to_string())?
        {
            Some(pkg) => pkg,
            None => self
                .store
                .assemble_context_package(task_id, None, Some(agent_id))
                .await
                .map_err(|e| e.to_string())?,
        };
        serde_json::to_string(&package).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Assemble and persist a fresh ContextPackage snapshot from current task state, decisions, artifacts, and handoffs. Requires authenticated Agent identity."
    )]
    async fn context_package_assemble(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_access(task_id, agent_id).await?;
        let package = self
            .store
            .assemble_context_package(task_id, None, Some(agent_id))
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&package).map_err(|e| e.to_string())
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for MorrowsMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "Morrows employee interface. The company control plane owns registration, fleet state, dispatch, assignment, Run creation, launch, cancellation, and scheduling. Employees may read their durable delivery inbox and acknowledge deliveries, read/reply to direct company conversations, maintain structured summaries for conversations addressed to them, access only work they own or have been assigned, pull/update working memory, receive instructions, collaborate, hand off work, renew an existing lease, and report progress or completion. Use the issued Bearer Agent credential; X-Agent-Instance-Id is an optional subject binding and must match when present. Loopback legacy mode may temporarily accept the identity header without a Bearer credential."
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
            "conversation_summary_get",
            "conversation_summary_revise",
            "delivery_inbox",
            "delivery_ack",
            "context_package_get",
            "context_package_assemble",
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

        let deliveries: Value = serde_json::from_str(
            &mcp.delivery_inbox(
                Parameters(DeliveryInboxRequest { limit: Some(20) }),
                Extension(parts(Some(a.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(deliveries.as_array().unwrap().len(), 1);
        let delivery_id = deliveries[0]["id"].as_str().unwrap().to_owned();
        assert!(
            mcp.delivery_ack(
                Parameters(DeliveryAckRequest {
                    delivery_id: delivery_id.clone(),
                }),
                Extension(parts(Some(b.id))),
            )
            .await
            .is_err()
        );
        let delivered: Value = serde_json::from_str(
            &mcp.delivery_ack(
                Parameters(DeliveryAckRequest { delivery_id }),
                Extension(parts(Some(a.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(delivered["status"], "delivered");

        store
            .create_human_conversation_message(conversation.id, "follow-up")
            .await
            .unwrap();
        assert_eq!(store.agent_delivery_inbox(a.id, 20).await.unwrap().len(), 1);

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
        mcp.conversation_get(
            Parameters(ConversationHistoryRequest {
                conversation_id: conversation.id.to_string(),
                before_message_id: None,
                after_message_id: None,
                limit: Some(20),
            }),
            Extension(parts(Some(a.id))),
        )
        .await
        .unwrap();
        assert!(
            store
                .agent_delivery_inbox(a.id, 20)
                .await
                .unwrap()
                .is_empty()
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

        // conversation_summary_get: other agent cannot access
        assert!(
            mcp.conversation_summary_get(
                Parameters(ConversationSummaryRequest {
                    conversation_id: conversation.id.to_string(),
                }),
                Extension(parts(Some(b.id))),
            )
            .await
            .is_err()
        );

        // conversation_summary_get: owner agent gets null when none exists
        let none_summary = mcp
            .conversation_summary_get(
                Parameters(ConversationSummaryRequest {
                    conversation_id: conversation.id.to_string(),
                }),
                Extension(parts(Some(a.id))),
            )
            .await
            .unwrap();
        assert_eq!(none_summary, "null");

        // Other agents cannot revise this conversation summary.
        assert!(
            mcp.conversation_summary_revise(
                Parameters(ReviseConversationSummaryRequest {
                    conversation_id: conversation.id.to_string(),
                    goal: "Wrong owner".into(),
                    current_state: "Should fail".into(),
                    important_findings: vec![],
                    decisions: vec![],
                    blockers: vec![],
                    unresolved_questions: vec![],
                    next_steps: vec![],
                    deterministic_facts: Default::default(),
                }),
                Extension(parts(Some(b.id))),
            )
            .await
            .is_err()
        );

        // The addressed agent can create a summary; Morrows pins it to the latest message.
        let rev: Value = serde_json::from_str(
            &mcp.conversation_summary_revise(
                Parameters(ReviseConversationSummaryRequest {
                    conversation_id: conversation.id.to_string(),
                    goal: "Resolve support inquiry".into(),
                    current_state: "Assisted".into(),
                    important_findings: vec!["Human asked for help".into()],
                    decisions: vec!["Responded directly".into()],
                    blockers: vec![],
                    unresolved_questions: vec![],
                    next_steps: vec!["Wait for follow-up".into()],
                    deterministic_facts: std::collections::BTreeMap::from([(
                        "reply_sent".into(),
                        "true".into(),
                    )]),
                }),
                Extension(parts(Some(a.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(rev["goal"], "Resolve support inquiry");
        assert_eq!(rev["created_by"], format!("agent:{}", a.id));
        assert!(rev["covers_until_message_id"].is_string());

        let got_summary: Value = serde_json::from_str(
            &mcp.conversation_summary_get(
                Parameters(ConversationSummaryRequest {
                    conversation_id: conversation.id.to_string(),
                }),
                Extension(parts(Some(a.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(got_summary["id"], rev["id"]);
        assert_eq!(got_summary["goal"], "Resolve support inquiry");
    }

    #[tokio::test]
    async fn instructions_get_consumes_matching_queued_deliveries() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store
            .register_agent("instruction-reader", &[])
            .await
            .unwrap();
        let task = store
            .create_task(
                serde_json::from_value(json!({
                    "title": "instruction delivery",
                    "description": "read instructions"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let assignment = store
            .claim_task(task.id, agent.id, "executor", 600)
            .await
            .unwrap();
        let profile = store
            .register_launch_profile(
                serde_json::from_value(json!({
                    "name": "instruction codex",
                    "adapter": "codex_cli",
                    "agent_instance_id": agent.id,
                    "program": "/bin/echo",
                    "default_cwd": "/tmp"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let attempt = store
            .enqueue_launch(
                serde_json::from_value(json!({
                    "assignment_id": assignment.id,
                    "launch_profile_id": profile.id
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .send_launch_instruction(
                morrows_core::SendLaunchInstruction {
                    launch_attempt_id: attempt.id,
                    body: "Read this through the legacy instruction API".into(),
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .agent_delivery_inbox(agent.id, 20)
                .await
                .unwrap()
                .len(),
            1
        );

        let mcp = MorrowsMcp::new(store.clone());
        let instructions: Value = serde_json::from_str(
            &mcp.instructions_get(
                Parameters(TaskIdRequest {
                    task_id: task.id.to_string(),
                }),
                Extension(parts(Some(agent.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(instructions.as_array().unwrap().len(), 1);
        assert!(
            store
                .agent_delivery_inbox(agent.id, 20)
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

    #[tokio::test]
    async fn context_package_mcp_tools_enforce_access_and_assemble() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = store.register_agent("agent-a", &[]).await.unwrap();
        let b = store.register_agent("agent-b", &[]).await.unwrap();
        let task = store
            .create_task(serde_json::from_value(json!({
                "title": "Build feature X",
                "description": "Implement feature X with full test coverage"
            })).unwrap())
            .await
            .unwrap();
        let assignment = store
            .claim_task(task.id, a.id, "executor", 300)
            .await
            .unwrap();
        let _ = store.start_run(assignment.id, a.id, None).await.unwrap();

        let mcp = MorrowsMcp::new(store.clone());

        // Unauthenticated -> error
        assert!(
            mcp.context_package_get(
                Parameters(TaskIdRequest {
                    task_id: task.id.to_string(),
                }),
                Extension(parts(None)),
            )
            .await
            .is_err()
        );

        // Unassigned agent b -> error
        assert!(
            mcp.context_package_get(
                Parameters(TaskIdRequest {
                    task_id: task.id.to_string(),
                }),
                Extension(parts(Some(b.id))),
            )
            .await
            .is_err()
        );

        // Assigned agent a -> auto-assembles context package if not exists
        let pkg_str = mcp
            .context_package_get(
                Parameters(TaskIdRequest {
                    task_id: task.id.to_string(),
                }),
                Extension(parts(Some(a.id))),
            )
            .await
            .unwrap();
        let pkg: Value = serde_json::from_str(&pkg_str).unwrap();
        assert_eq!(
            pkg["objective"],
            "Build feature X: Implement feature X with full test coverage"
        );
        assert_eq!(pkg["work_item_id"], task.id.to_string());

        // Calling context_package_assemble re-assembles fresh
        let fresh_str = mcp
            .context_package_assemble(
                Parameters(TaskIdRequest {
                    task_id: task.id.to_string(),
                }),
                Extension(parts(Some(a.id))),
            )
            .await
            .unwrap();
        let fresh_pkg: Value = serde_json::from_str(&fresh_str).unwrap();
        assert_eq!(fresh_pkg["work_item_id"], task.id.to_string());
    }
}
