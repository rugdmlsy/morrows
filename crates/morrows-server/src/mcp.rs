use axum::http::request::Parts;
use morrows_core::{
    CreateArtifact, CreateDecision, CreateHandoff, CreateMessage, CreateSessionSummaryRevision,
    CreateThread, SessionHistoryRequest, SessionReply,
};
use morrows_core::{CreateContextRevision, CreateTask, Id, TaskQuery, TaskState};
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

    async fn ensure_task_read_access(&self, task_id: Id, agent_id: Id) -> Result<(), String> {
        self.store
            .get_agent(agent_id)
            .await
            .map_err(|e| e.to_string())?;
        self.store
            .get_task(task_id)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Assemble a live reading view without persisting or acknowledging anything.
    /// The full task/current revision is authoritative; historical collections are
    /// bounded and carry offsets so missing context is never silently discarded.
    /// Persisted packages remain separate immutable evidence and may be older.
    async fn load_task_context(&self, task_id: Id, agent_id: Id) -> Result<String, String> {
        let task = self
            .store
            .get_task(task_id)
            .await
            .map_err(|e| e.to_string())?;
        let project = match task.project_id {
            Some(id) => Some(
                self.store
                    .get_project(id)
                    .await
                    .map_err(|e| e.to_string())?,
            ),
            None => None,
        };
        let context = if task.current_context_revision_id.is_some() {
            Some(
                self.store
                    .get_current_context(task_id)
                    .await
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        let memory = self
            .store
            .context_memories_page(Some(task_id), task.project_id, Some(agent_id), false, 20, 0)
            .await
            .map_err(|e| e.to_string())?;
        let instructions = self
            .store
            .task_launch_instructions_page(task_id, 20, 0)
            .await
            .map_err(|e| e.to_string())?;
        let mut collaboration = serde_json::Map::new();
        for section in ["handoffs", "decisions", "artifacts", "dependencies"] {
            let page = self
                .store
                .task_collaboration_page(task_id, Some(section), 5, 0)
                .await
                .map_err(|e| e.to_string())?;
            collaboration.insert(section.into(), page[section].clone());
        }
        let mut missing = Vec::new();
        if task.description.trim().is_empty() {
            missing.push("task_description");
        }
        if project
            .as_ref()
            .is_none_or(|p| p.description.trim().is_empty())
        {
            missing.push("project_background");
        }
        if context
            .as_ref()
            .is_none_or(|c| c.background.trim().is_empty())
        {
            missing.push("context_background");
        }
        // Imported research tasks also name explicit acceptance gates
        // `freeze_requires`. Point to the original fields without rewriting or
        // duplicating criteria, and do not interpret them as passed checks.
        let acceptance_paths: Vec<_> = ["acceptance_criteria", "freeze_requires"]
            .into_iter()
            .filter(|key| {
                context
                    .as_ref()
                    .and_then(|c| c.constraints.get(*key))
                    .is_some_and(|v| match v {
                        Value::String(s) => !s.trim().is_empty(),
                        Value::Array(a) => !a.is_empty(),
                        Value::Object(o) => !o.is_empty(),
                        _ => false,
                    })
            })
            .map(|key| format!("context.constraints.{key}"))
            .collect();
        if acceptance_paths.is_empty() {
            missing.push("structured_acceptance_criteria");
        }
        let latest_package = self
            .store
            .get_latest_context_package(task_id)
            .await
            .map_err(|e| e.to_string())?;
        let execution = self
            .store
            .task_execution_page(task_id, agent_id, 5, 0)
            .await
            .map_err(|e| e.to_string())?;
        let package_ref = latest_package.map(|p| json!({
            "id": p.id, "created_at": p.created_at,
            "context_snapshot_id": p.context_snapshot_id,
            "context_matches_current": p.context_snapshot_id == task.current_context_revision_id,
        }));
        Ok(json!({
            "task": task, "project": project, "context": context,
            "memory": memory, "instructions": instructions, "collaboration": collaboration,
            "missing_context": missing, "persisted_package": package_ref, "execution": execution,
            "acceptance_criteria_paths": acceptance_paths,
            "read_more": {"memory": "memory_get", "instructions": "instructions_get", "collaboration": "task_collaboration", "events": "task_events", "execution": "task_get"},
        }).to_string())
    }

    async fn ensure_task_write_access(&self, task_id: Id, agent_id: Id) -> Result<(), String> {
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
        if self
            .store
            .agent_has_open_task_session(task_id, agent_id)
            .await
            .map_err(|e| e.to_string())?
        {
            return Ok(());
        }
        Err(
            "task is not owned by, assigned to, or shared through an open Task Session with the authenticated agent instance"
                .into(),
        )
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskIdRequest {
    pub task_id: String,
}

fn default_limit() -> i64 {
    20
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PageRequest {
    /// Page size, 1..100. Defaults to 20.
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default)]
    pub offset: i64,
}

impl Default for PageRequest {
    fn default() -> Self {
        Self {
            limit: default_limit(),
            offset: 0,
        }
    }
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TaskListScope {
    #[default]
    Assigned,
    All,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct TaskListRequest {
    /// Defaults to assigned; all explicitly searches every task.
    #[serde(default)]
    pub scope: TaskListScope,
    /// In assigned scope, defaults to the authenticated agent.
    pub agent_instance_id: Option<String>,
    pub project_id: Option<String>,
    /// backlog, ready, in_progress, review, blocked, done, or cancelled.
    pub state: Option<String>,
    /// Include done/cancelled tasks. An explicit state overrides this flag.
    #[serde(default)]
    pub include_completed: bool,
    #[serde(flatten)]
    pub page: PageRequest,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct TaskPageRequest {
    pub task_id: String,
    #[serde(flatten)]
    pub page: PageRequest,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct MemoryRequest {
    pub task_id: String,
    /// Omit for the current context; select an immutable revision belonging to this task.
    pub context_revision_id: Option<String>,
    /// Include superseded visible long-term memories, retaining original content and provenance.
    #[serde(default)]
    pub include_superseded: bool,
    /// Also return a newest-first page of full context revisions, with its own next_offset.
    #[serde(default)]
    pub include_context_history: bool,
    /// Applies independently to long-term memory and requested context history.
    #[serde(flatten)]
    pub page: PageRequest,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct CollaborationRequest {
    pub task_id: String,
    /// handoffs, artifacts, decisions, threads, messages, or dependencies; omitted returns all sections.
    pub section: Option<String>,
    #[serde(flatten)]
    pub page: PageRequest,
}

#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct EventsRequest {
    pub task_id: String,
    pub event_type: Option<String>,
    #[serde(flatten)]
    pub page: PageRequest,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ProjectRequest {
    pub project_id: String,
    /// Include superseded shared memory entries; defaults to current knowledge only.
    #[serde(default)]
    pub include_superseded: bool,
    /// Pagination applies to project/organization memory, not the project description.
    #[serde(flatten)]
    pub page: PageRequest,
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
pub struct SessionSummaryRequest {
    pub session_id: String,
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
pub struct ReviseSessionSummaryRequest {
    pub session_id: String,
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
        description = "Identify the authenticated agent and the default discovery scope. Does not return credentials."
    )]
    async fn whoami(&self, Extension(parts): Extension<Parts>) -> Result<String, String> {
        let agent = self
            .store
            .get_agent(authenticated_agent(&parts)?)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "agent_instance_id": agent.id, "name": agent.name,
            "display_name": agent.display_name, "status": agent.status,
            "default_task_scope": "assigned", "can_query_other_tasks": true,
            "start_here": "task_list; use scope=all to discover other tasks; then task_context"
        })
        .to_string())
    }

    #[tool(
        description = "List compact task summaries. Defaults to unfinished tasks assigned to the caller, including expired leases. Use scope=all for all tasks, or agent_instance_id for another agent. Filter by project_id/state and follow next_offset."
    )]
    async fn task_list(
        &self,
        Parameters(req): Parameters<TaskListRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let caller = authenticated_agent(&parts)?;
        self.store
            .get_agent(caller)
            .await
            .map_err(|e| e.to_string())?;
        let agent_id = match req.scope {
            TaskListScope::Assigned => Some(
                req.agent_instance_id
                    .as_deref()
                    .map(parse_id)
                    .transpose()?
                    .unwrap_or(caller),
            ),
            TaskListScope::All if req.agent_instance_id.is_some() => {
                return Err("agent_instance_id requires scope=assigned".into());
            }
            TaskListScope::All => None,
        };
        let project_id = req.project_id.as_deref().map(parse_id).transpose()?;
        let state = req
            .state
            .as_deref()
            .map(str::parse::<TaskState>)
            .transpose()
            .map_err(|e| e.to_string())?;
        let tasks = self
            .store
            .query_tasks(
                &TaskQuery {
                    agent_instance_id: agent_id,
                    project_id,
                    state,
                    include_completed: req.include_completed,
                },
                req.page.limit,
                req.page.offset,
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "caller_agent_instance_id": caller,
            "filters": {"scope": if agent_id.is_some() { "assigned" } else { "all" }, "agent_instance_id": agent_id,
                "project_id": project_id, "state": state, "include_completed": req.include_completed},
            "items": tasks.items, "next_offset": tasks.next_offset,
            "empty_reason": if !tasks.items.is_empty() { None } else if req.page.offset > 0 {
                Some("No tasks at this offset. Restart at offset=0 with the same filters to refresh this live listing.")
            } else { Some("No tasks match these filters; this is not a permission restriction. Use scope=all or include_completed=true to broaden discovery.") },
        }).to_string())
    }

    #[tool(
        description = "List all project summaries, with description_preview capped at 240 characters and an explicit description_truncated flag. Follow next_offset; project_get returns full background and shared memory."
    )]
    async fn project_list(
        &self,
        Parameters(req): Parameters<PageRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        self.store
            .get_agent(authenticated_agent(&parts)?)
            .await
            .map_err(|e| e.to_string())?;
        let page = self
            .store
            .projects_page(req.limit, req.offset)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&page).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read any project's full background and a page of shared organization/project memory. Defaults to current memory; include_superseded exposes history with provenance. Use task_list(scope=all, project_id=...) to discover its work."
    )]
    async fn project_get(
        &self,
        Parameters(req): Parameters<ProjectRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        self.store
            .get_agent(authenticated_agent(&parts)?)
            .await
            .map_err(|e| e.to_string())?;
        let project_id = parse_id(&req.project_id)?;
        let project = self
            .store
            .get_project(project_id)
            .await
            .map_err(|e| e.to_string())?;
        let memory = self
            .store
            .context_memories_page(
                None,
                Some(project_id),
                None,
                req.include_superseded,
                req.page.limit,
                req.page.offset,
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({"project": project, "memory": memory, "include_superseded": req.include_superseded}).to_string())
    }

    #[tool(
        description = "Start work here: read fresh task/project background, current context and memory, latest handoff, decisions, artifacts, dependencies and instructions. Reports missing background/structured acceptance criteria. Bounded sections include continuation offsets; no message/event history and no writes."
    )]
    async fn task_context(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_read_access(task_id, agent_id).await?;
        self.load_task_context(task_id, agent_id).await
    }

    #[tool(
        description = "List direct company sessions with queued human messages addressed to the authenticated employee. Returns summaries only; use session_get to load one history."
    )]
    async fn session_inbox(&self, Extension(parts): Extension<Parts>) -> Result<String, String> {
        let value = self
            .store
            .agent_session_inbox(authenticated_agent(&parts)?)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read one direct company session addressed to the authenticated employee. History is paged; before_message_id loads older messages and after_message_id loads newer messages."
    )]
    async fn session_get(
        &self,
        Parameters(req): Parameters<SessionHistoryRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let before = req.before_message_id.as_deref().map(parse_id).transpose()?;
        let after = req.after_message_id.as_deref().map(parse_id).transpose()?;
        let session_id = parse_id(&req.session_id)?;
        let agent_id = authenticated_agent(&parts)?;
        let value = self
            .store
            .agent_session_history(session_id, agent_id, before, after, req.limit.unwrap_or(80))
            .await
            .map_err(|e| e.to_string())?;
        self.store
            .acknowledge_session_deliveries(
                session_id,
                agent_id,
                &format!("session_get:{agent_id}"),
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Reply to a direct company session addressed to the authenticated employee. The reply marks queued human messages in that session delivered."
    )]
    async fn session_reply(
        &self,
        Parameters(req): Parameters<SessionReply>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let value = self
            .store
            .agent_reply_session(
                parse_id(&req.session_id)?,
                authenticated_agent(&parts)?,
                &req.body,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read the latest structured summary revision for a direct company session addressed to the authenticated employee."
    )]
    async fn session_summary_get(
        &self,
        Parameters(req): Parameters<SessionSummaryRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let session_id = parse_id(&req.session_id)?;
        let agent_id = authenticated_agent(&parts)?;
        let session = self
            .store
            .get_session(session_id)
            .await
            .map_err(|e| e.to_string())?;
        if session.agent_instance_id != agent_id {
            return Err("session belongs to another agent instance".into());
        }
        let summary = self
            .store
            .get_latest_session_summary_revision(session_id)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&summary).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Create a new structured summary revision for a direct company session addressed to the authenticated employee. Morrows automatically links the previous summary revision and covers the latest message currently in the session."
    )]
    async fn session_summary_revise(
        &self,
        Parameters(req): Parameters<ReviseSessionSummaryRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let session_id = parse_id(&req.session_id)?;
        let agent_id = authenticated_agent(&parts)?;
        let session = self
            .store
            .get_session(session_id)
            .await
            .map_err(|e| e.to_string())?;
        if session.agent_instance_id != agent_id {
            return Err("session belongs to another agent instance".into());
        }

        let previous_revision_id = self
            .store
            .get_latest_session_summary_revision(session_id)
            .await
            .map_err(|e| e.to_string())?
            .map(|summary| summary.id);
        let history = self
            .store
            .session_history(session_id, None, None, 1)
            .await
            .map_err(|e| e.to_string())?;
        let covers_until_message_id = history.messages.last().map(|message| message.id);

        let summary = self
            .store
            .create_session_summary_revision(CreateSessionSummaryRevision {
                session_id,
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
        description = "List durable Morrows deliveries queued for the authenticated employee. A delivery references an existing session message or launch instruction; process the referenced source and then call delivery_ack."
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
        description = "Acknowledge one durable Morrows delivery after the authenticated employee or its provider bridge has received it. A delivery can only be acknowledged by its target AgentInstance."
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
        description = "Read management instructions for any task. Only the caller's deliveries are acknowledged."
    )]
    async fn instructions_get(
        &self,
        Parameters(req): Parameters<TaskPageRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_read_access(task_id, agent_id).await?;
        let value = self
            .store
            .task_launch_instructions_page(task_id, req.page.limit, req.page.offset)
            .await
            .map_err(|e| e.to_string())?;
        self.store
            .acknowledge_instruction_deliveries_for_ids(
                &value
                    .items
                    .iter()
                    .map(|instruction| instruction.id)
                    .collect::<Vec<_>>(),
                agent_id,
                &format!("instructions_get:{agent_id}"),
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Atomically accept a pending handoff with a live same-task target run owned by the caller."
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
    #[tool(description = "Create a durable artifact.")]

    async fn artifact_create(
        &self,
        Parameters(req): Parameters<CreateArtifactRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_write_access(task_id, agent_id).await?;
        let value = self
            .store
            .create_artifact(task_id, agent_id, req.input)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "Create a durable decision.")]
    async fn decision_create(
        &self,
        Parameters(req): Parameters<CreateDecisionRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_write_access(task_id, agent_id).await?;
        let value = self
            .store
            .create_decision(task_id, agent_id, req.input)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "Create a durable thread.")]
    async fn thread_create(
        &self,
        Parameters(req): Parameters<CreateThreadRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_write_access(task_id, agent_id).await?;
        let value = self
            .store
            .create_thread(task_id, agent_id, req.input)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }
    #[tool(description = "Create a durable message.")]
    async fn message_create(
        &self,
        Parameters(req): Parameters<CreateMessageRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let thread_id = parse_id(&req.thread_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_write_access(task_id, agent_id).await?;
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
        description = "Create a durable handoff. Atomically ends source runs and releases assignment; requires task context."
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
        description = "Read bounded collaboration pages for any task, newest first. Select a section and follow its next_offset to load more."
    )]
    async fn task_collaboration(
        &self,
        Parameters(req): Parameters<CollaborationRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_read_access(task_id, agent_id).await?;
        let value = self
            .store
            .task_collaboration_page(
                task_id,
                req.section.as_deref(),
                req.page.limit,
                req.page.offset,
            )
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
        self.ensure_task_read_access(value.handoff.task_id, agent_id)
            .await?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read any task by ID with paged assignment metadata and the caller's executions/checkpoints. Use task_context for project background and working context."
    )]
    async fn task_get(
        &self,
        Parameters(req): Parameters<TaskPageRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_read_access(task_id, agent_id).await?;
        let task = self
            .store
            .get_task(task_id)
            .await
            .map_err(|e| e.to_string())?;
        let execution = self
            .store
            .task_execution_page(task_id, agent_id, req.page.limit, req.page.offset)
            .await
            .map_err(|e| e.to_string())?;
        let mut value = serde_json::to_value(&task).map_err(|e| e.to_string())?;
        value["execution"] = execution;
        Ok(value.to_string())
    }

    #[tool(
        description = "Submit a work request to Morrows for company scheduling. The caller becomes the request owner; employees cannot set dispatch priority or claim the work themselves."
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
        description = "Read a task's current context and visible long-term memory. include_superseded retrieves replaced memory; include_context_history pages full context revisions; context_revision_id selects one revision. Each history keeps its provenance. Message history is available through task_collaboration."
    )]
    async fn memory_get(
        &self,
        Parameters(req): Parameters<MemoryRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_read_access(task_id, agent_id).await?;
        let task = self
            .store
            .get_task(task_id)
            .await
            .map_err(|e| e.to_string())?;
        let revision_id = req
            .context_revision_id
            .as_deref()
            .map(parse_id)
            .transpose()?
            .or(task.current_context_revision_id);
        let context = match revision_id {
            Some(id) => Some(
                self.store
                    .get_context_revision(id)
                    .await
                    .map_err(|e| e.to_string())?,
            ),
            None => None,
        };
        if context.as_ref().is_some_and(|c| c.task_id != task_id) {
            return Err("context_revision_id does not belong to task_id".into());
        }
        let long_term_memory = self
            .store
            .context_memories_page(
                Some(task_id),
                task.project_id,
                Some(agent_id),
                req.include_superseded,
                req.page.limit,
                req.page.offset,
            )
            .await
            .map_err(|e| e.to_string())?;
        let mut result = json!({
            "task_id": task.id,
            "long_term_memory": long_term_memory.items, "next_offset": long_term_memory.next_offset,
            "context": context, "current_context_revision_id": task.current_context_revision_id,
            "context_is_current": revision_id == task.current_context_revision_id,
            "include_superseded": req.include_superseded,
        });
        // Context revisions and durable memory evolve independently. Selecting an
        // old context does not reconstruct memory at that time; history is explicit
        // and each collection carries its own continuation offset and provenance.
        if req.include_context_history {
            result["context_history"] = serde_json::to_value(
                self.store
                    .context_revisions_page(task_id, req.page.limit, req.page.offset)
                    .await
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        }
        Ok(result.to_string())
    }

    #[tool(
        description = "Write a new immutable working-memory revision for a work item owned by, assigned to, or explicitly shared through an open Task Session with the authenticated employee."
    )]
    async fn memory_revise(
        &self,
        Parameters(req): Parameters<ReviseContextRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_write_access(task_id, agent_id).await?;
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
        description = "Read a page of task events, newest first. Optionally filter by event_type; follow next_offset."
    )]
    async fn task_events(
        &self,
        Parameters(req): Parameters<EventsRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_read_access(task_id, agent_id).await?;
        let events = self
            .store
            .task_events_page(
                task_id,
                req.event_type.as_deref(),
                req.page.limit,
                req.page.offset,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&events).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read the latest persisted context package for any task; may be stale. If absent, returns null without writing. Start with task_context for fresh background."
    )]
    async fn context_package_get(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_read_access(task_id, agent_id).await?;
        let package = self
            .store
            .get_latest_context_package(task_id)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&package).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Assemble and persist a fresh ContextPackage snapshot from current task state, decisions, artifacts, and handoffs."
    )]
    async fn context_package_assemble(
        &self,
        Parameters(req): Parameters<TaskIdRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let task_id = parse_id(&req.task_id)?;
        let agent_id = authenticated_agent(&parts)?;
        self.ensure_task_write_access(task_id, agent_id).await?;
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
            .with_instructions(include_str!("mcp_instructions.md"))
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
#[path = "mcp_discovery_tests.rs"]
mod discovery_tests;

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
    async fn task_scoped_session_grants_explicit_task_access() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("session-reader", &[]).await.unwrap();
        let task = store
            .create_task(
                serde_json::from_value(json!({
                    "title": "Scoped task",
                    "description": "Share through a Session"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let mcp = MorrowsMcp::new(store.clone());

        assert!(
            mcp.ensure_task_write_access(task.id, agent.id)
                .await
                .is_err()
        );

        let session = store
            .create_scoped_session(
                morrows_core::CreateSession {
                    agent_instance_id: agent.id,
                    title: "Task discussion".into(),
                },
                None,
                Some(task.id),
            )
            .await
            .unwrap();
        assert!(
            mcp.ensure_task_write_access(task.id, agent.id)
                .await
                .is_ok()
        );

        store
            .update_session_scope(
                session.id,
                morrows_core::UpdateSessionScope {
                    project_id: None,
                    task_id: None,
                },
            )
            .await
            .unwrap();
        assert!(
            mcp.ensure_task_write_access(task.id, agent.id)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn mcp_surface_is_employee_only() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let mcp = MorrowsMcp::new(store);
        for name in [
            "session_inbox",
            "session_get",
            "session_reply",
            "work_request_submit",
            "task_get",
            "whoami",
            "task_list",
            "project_list",
            "project_get",
            "task_context",
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
            "session_summary_get",
            "session_summary_revise",
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
            "conversation_inbox",
            "conversation_get",
            "conversation_reply",
            "conversation_summary_get",
            "conversation_summary_revise",
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
    async fn direct_sessions_are_scoped_to_the_addressed_employee() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let a = store.register_agent("chat-a", &[]).await.unwrap();
        let b = store.register_agent("chat-b", &[]).await.unwrap();
        let session = store
            .create_session(morrows_core::CreateSession {
                agent_instance_id: a.id,
                title: "Direct".into(),
            })
            .await
            .unwrap();
        store
            .create_human_session_message(session.id, "hello")
            .await
            .unwrap();
        let mcp = MorrowsMcp::new(store.clone());

        let inbox: Value = serde_json::from_str(
            &mcp.session_inbox(Extension(parts(Some(a.id))))
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
            .create_human_session_message(session.id, "follow-up")
            .await
            .unwrap();
        assert_eq!(store.agent_delivery_inbox(a.id, 20).await.unwrap().len(), 1);

        assert!(
            mcp.session_get(
                Parameters(SessionHistoryRequest {
                    session_id: session.id.to_string(),
                    before_message_id: None,
                    after_message_id: None,
                    limit: Some(20),
                }),
                Extension(parts(Some(b.id))),
            )
            .await
            .is_err()
        );
        mcp.session_get(
            Parameters(SessionHistoryRequest {
                session_id: session.id.to_string(),
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

        mcp.session_reply(
            Parameters(SessionReply {
                session_id: session.id.to_string(),
                body: "hi".into(),
            }),
            Extension(parts(Some(a.id))),
        )
        .await
        .unwrap();
        assert!(store.agent_session_inbox(a.id).await.unwrap().is_empty());

        // session_summary_get: other agent cannot access
        assert!(
            mcp.session_summary_get(
                Parameters(SessionSummaryRequest {
                    session_id: session.id.to_string(),
                }),
                Extension(parts(Some(b.id))),
            )
            .await
            .is_err()
        );

        // session_summary_get: owner agent gets null when none exists
        let none_summary = mcp
            .session_summary_get(
                Parameters(SessionSummaryRequest {
                    session_id: session.id.to_string(),
                }),
                Extension(parts(Some(a.id))),
            )
            .await
            .unwrap();
        assert_eq!(none_summary, "null");

        // Other agents cannot revise this session summary.
        assert!(
            mcp.session_summary_revise(
                Parameters(ReviseSessionSummaryRequest {
                    session_id: session.id.to_string(),
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
            &mcp.session_summary_revise(
                Parameters(ReviseSessionSummaryRequest {
                    session_id: session.id.to_string(),
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
            &mcp.session_summary_get(
                Parameters(SessionSummaryRequest {
                    session_id: session.id.to_string(),
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

        store
            .send_launch_instruction(
                morrows_core::SendLaunchInstruction {
                    launch_attempt_id: attempt.id,
                    body: "Second page".into(),
                },
                None,
            )
            .await
            .unwrap();
        let other = store.register_agent("other-reader", &[]).await.unwrap();
        let mcp = MorrowsMcp::new(store.clone());
        // Inspecting someone else's task must not consume their deliveries.
        mcp.instructions_get(
            Parameters(TaskPageRequest {
                task_id: task.id.to_string(),
                ..Default::default()
            }),
            Extension(parts(Some(other.id))),
        )
        .await
        .unwrap();
        mcp.task_context(
            Parameters(TaskIdRequest {
                task_id: task.id.to_string(),
            }),
            Extension(parts(Some(agent.id))),
        )
        .await
        .unwrap();
        assert_eq!(
            store
                .agent_delivery_inbox(agent.id, 20)
                .await
                .unwrap()
                .len(),
            2
        );
        let instructions: Value = serde_json::from_str(
            &mcp.instructions_get(
                Parameters(TaskPageRequest {
                    task_id: task.id.to_string(),
                    page: PageRequest {
                        limit: 1,
                        offset: 0,
                    },
                }),
                Extension(parts(Some(agent.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(instructions["items"].as_array().unwrap().len(), 1);
        assert_eq!(instructions["next_offset"], 1);
        assert_eq!(
            store
                .agent_delivery_inbox(agent.id, 20)
                .await
                .unwrap()
                .len(),
            1
        );
        mcp.instructions_get(
            Parameters(TaskPageRequest {
                task_id: task.id.to_string(),
                page: PageRequest {
                    limit: 1,
                    offset: 1,
                },
            }),
            Extension(parts(Some(agent.id))),
        )
        .await
        .unwrap();
        assert!(
            store
                .agent_delivery_inbox(agent.id, 20)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn memory_get_materializes_project_long_term_memory() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("memory-reader", &[]).await.unwrap();
        let project = store
            .create_project(morrows_core::CreateProject {
                name: "Memory project".into(),
                description: String::new(),
            })
            .await
            .unwrap();
        let task = store
            .create_task(CreateTask {
                project_id: Some(project.id),
                title: "Read project memory".into(),
                description: String::new(),
                owner_actor_id: format!("agent:{}", agent.id),
                state: TaskState::Ready,
                priority: 0,
            })
            .await
            .unwrap();
        store
            .create_memory_entry(morrows_core::CreateMemoryEntry {
                scope_type: "project".into(),
                project_id: Some(project.id),
                agent_instance_id: None,
                task_id: None,
                title: "Imported project memory".into(),
                content: json!({"constraint":"preserve project context"}),
                source_kind: "chatgpt_history_import".into(),
                source_ref: Some("chatgpt:test".into()),
                visibility: "shared".into(),
                supersedes_memory_id: None,
            })
            .await
            .unwrap();

        let mcp = MorrowsMcp::new(store);
        let value: Value = serde_json::from_str(
            &mcp.memory_get(
                Parameters(MemoryRequest {
                    task_id: task.id.to_string(),
                    ..Default::default()
                }),
                Extension(parts(Some(agent.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();

        assert_eq!(value["long_term_memory"].as_array().unwrap().len(), 1);
        assert_eq!(
            value["long_term_memory"][0]["content"]["constraint"],
            "preserve project context"
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
                Parameters(CollaborationRequest {
                    task_id: "bad-id".into(),
                    ..Default::default()
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
            .create_task(
                serde_json::from_value(json!({
                    "title": "Build feature X",
                    "description": "Implement feature X with full test coverage"
                }))
                .unwrap(),
            )
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

        // Unassigned readers can inspect packages, but a read creates no snapshot.
        assert_eq!(
            mcp.context_package_get(
                Parameters(TaskIdRequest {
                    task_id: task.id.to_string(),
                }),
                Extension(parts(Some(b.id))),
            )
            .await
            .unwrap(),
            "null"
        );
        assert!(
            store
                .get_latest_context_package(task.id)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            mcp.context_package_assemble(
                Parameters(TaskIdRequest {
                    task_id: task.id.to_string()
                }),
                Extension(parts(Some(b.id))),
            )
            .await
            .is_err()
        );

        // Only an explicit authorized assembly persists a package.
        let pkg_str = mcp
            .context_package_assemble(
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
        let read_back: Value = serde_json::from_str(
            &mcp.context_package_get(
                Parameters(TaskIdRequest {
                    task_id: task.id.to_string(),
                }),
                Extension(parts(Some(b.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(read_back["id"], fresh_pkg["id"]);
    }
}
