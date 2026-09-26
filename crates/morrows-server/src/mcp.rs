use crate::memory_search::MemorySearch;
use axum::http::request::Parts;
use morrows_core::{
    CreateArtifact, CreateDecision, CreateHandoff, CreateMessage, CreateSessionSummaryRevision,
    CreateThread, SessionHistoryRequest, SessionReply,
};
use morrows_core::{CreateTask, Id, TaskQuery, TaskState, UpdateContextRevision};
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
    memory_search: Option<MemorySearch>,
    tool_router: ToolRouter<Self>,
}

impl MorrowsMcp {
    #[cfg(test)]
    pub fn new(store: Store) -> Self {
        Self::new_with_memory_search(store, None)
    }

    pub fn new_with_memory_search(store: Store, memory_search: Option<MemorySearch>) -> Self {
        Self {
            store,
            memory_search,
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
        let retrieval_query = format!(
            "Task: {}
Description: {}
Goal: {}
Current summary: {}",
            task.title,
            task.description,
            context.as_ref().map(|c| c.goal.as_str()).unwrap_or(""),
            context
                .as_ref()
                .map(|c| c.current_summary.as_str())
                .unwrap_or("")
        );
        let memory_retrieval = match (&self.memory_search, task.project_id) {
            (Some(search), Some(project_id)) => {
                search
                    .default_project_retrieval(&self.store, project_id, &retrieval_query)
                    .await
            }
            (None, Some(_)) => json!({
                "available": false,
                "engine": "ripgrep",
                "reason": "ripgrep_not_configured",
                "fallback": "task_context.memory",
            }),
            (_, None) => json!({
                "available": false,
                "reason": "task_has_no_project",
                "fallback": "task_context.memory",
            }),
        };
        let package_ref = latest_package.map(|p| json!({
            "id": p.id, "created_at": p.created_at,
            "context_snapshot_id": p.context_snapshot_id,
            "context_matches_current": p.context_snapshot_id == task.current_context_revision_id,
        }));
        let requests = self
            .store
            .assignment_requests_page(Some(task_id), Some(agent_id), true, 5, 0)
            .await
            .map_err(|e| e.to_string())?;
        Ok(json!({
            "task": task, "project": project, "context": context,
            "memory": memory, "memory_retrieval": memory_retrieval,
            "instructions": instructions, "collaboration": collaboration,
            "missing_context": missing, "persisted_package": package_ref, "execution": execution,
            "acceptance_criteria_paths": acceptance_paths,
            "assignment_requests": requests,
            "workflow": {"request_assignment":"task_request_assignment", "request_status":"assignment_request_list", "persist_milestone":"run_milestone", "handoff_requires_latest_milestone":true, "publish_project_knowledge":"project_memory_publish", "completion_preflight":"run_completion_check", "structured_completion_required_for_executor":context.as_ref().is_some_and(|c| !morrows_core::completion_criteria(&c.constraints).is_empty()), "project_memory_disposition_required_for_executor":task.project_id.is_some()},
            "read_more": {"memory": "memory_get", "memory_search": "memory_search", "instructions": "instructions_get", "collaboration": "task_collaboration", "events": "task_events", "execution": "task_get"},
        }).to_string())
    }

    async fn ensure_task_write_access(&self, task_id: Id, agent_id: Id) -> Result<(), String> {
        self.store
            .ensure_task_write_access(task_id, agent_id)
            .await
            .map_err(|e| e.to_string())
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskIdRequest {
    pub task_id: String,
}
#[derive(Debug, Deserialize, JsonSchema)]
pub struct MemorySearchRequest {
    pub task_id: String,
    pub query: String,
    #[serde(default = "default_memory_search_top_k")]
    pub top_k: usize,
}

fn default_memory_search_top_k() -> usize {
    8
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
pub struct MilestoneRunRequest {
    pub run_id: String,
    pub input: morrows_core::CreateRunMilestone,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompleteRunRequest {
    pub run_id: String,
    #[serde(default)]
    #[schemars(with = "std::collections::BTreeMap<String, Value>")]
    pub result: Value,
    /// Required for executor tasks with acceptance_criteria/freeze_requires. Obtain the template from run_completion_check.
    pub completion: Option<morrows_core::CompletionReport>,
    /// Required for executor tasks attached to a project. Publish/update reusable project knowledge or explicitly use not_applicable with a rationale.
    pub memory_disposition: Option<morrows_core::MemoryDisposition>,
}

impl CompleteRunRequest {
    fn result_with_completion(&self) -> Result<Value, String> {
        let mut result = self.result.clone();
        if self.completion.is_some() || self.memory_disposition.is_some() {
            if result.is_null() {
                result = json!({});
            }
            let object = result.as_object_mut().ok_or(
                "result must be an object when completion or memory_disposition is supplied",
            )?;
            if let Some(completion) = &self.completion {
                if object.contains_key("completion") {
                    return Err(
                        "supply completion either at top level or in result, not both".into(),
                    );
                }
                object.insert(
                    "completion".into(),
                    serde_json::to_value(completion).map_err(|e| e.to_string())?,
                );
            }
            if let Some(disposition) = &self.memory_disposition {
                if object.contains_key("memory_disposition") {
                    return Err(
                        "supply memory_disposition either at top level or in result, not both"
                            .into(),
                    );
                }
                object.insert(
                    "memory_disposition".into(),
                    serde_json::to_value(disposition).map_err(|e| e.to_string())?,
                );
            }
        }
        Ok(result)
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AssignmentRequestInput {
    pub task_id: String,
    pub role: String,
    pub reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AssignmentRequestList {
    pub task_id: Option<String>,
    #[serde(default)]
    pub include_resolved: bool,
    #[serde(flatten)]
    pub page: PageRequest,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WithdrawAssignmentRequest {
    pub request_id: String,
    pub reason: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReviseContextRequest {
    pub task_id: String,
    /// Omitted fields retain their current values; provided strings replace them.
    pub goal: Option<String>,
    pub background: Option<String>,
    /// Recursively merge object keys. Explicit arrays/scalars/null replace that key's value; unmentioned keys survive.
    #[schemars(with = "Option<std::collections::BTreeMap<String, Value>>")]
    pub constraints: Option<serde_json::Map<String, Value>>,
    pub current_summary: Option<String>,
    /// Use the revision ID you read to reject an update based on stale context.
    pub expected_context_revision_id: Option<String>,
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
        if self
            .store
            .project_memory_head(project_id)
            .await
            .map_err(|e| e.to_string())?
            != project.memory_head
        {
            return Err("project memory changed while reading; retry project_get".into());
        }
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
        description = "Create a durable handoff. Atomically ends source runs and releases assignment. New handoffs must cite the source Run's latest milestone; that milestone must pin the current task context and already contain every referenced artifact/decision."
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
        description = "Read any task by ID with paged assignment metadata, task-wide immutable milestones, and the caller's private Run/checkpoint details. Milestones are visible immediately as task execution history and do not require project-memory publication. Successor recovery includes the predecessor's latest milestone when available. Use task_context for project background and working context."
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
        description = "Persist an immutable Run milestone and atomically refresh the Run's latest checkpoint. Use after a substantive subgoal, before a long/risky operation, when provider/token budget is under pressure, and immediately before handoff. Include exact next_step, ordered next_plan, execution_locations, and relevant same-task evidence IDs."
    )]
    async fn run_milestone(
        &self,
        Parameters(req): Parameters<MilestoneRunRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let milestone = self
            .store
            .create_run_milestone(
                parse_id(&req.run_id)?,
                authenticated_agent(&parts)?,
                req.input,
            )
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&milestone).map_err(|e| e.to_string())
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
        description = "Complete an owned run. Project-backed executor tasks must explicitly supply memory_disposition: published/updated with same-task project MemoryEntry IDs, or not_applicable with a rationale. Executor tasks with acceptance_criteria/freeze_requires also require a current-context completion report with every criterion passed, rationale and same-task artifact references. Use run_completion_check first. Validation is atomic; failure leaves state unchanged."
    )]
    async fn run_complete(
        &self,
        Parameters(req): Parameters<CompleteRunRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let agent_id = authenticated_agent(&parts)?;
        let result = req.result_with_completion()?;
        let run = self
            .store
            .complete_run(parse_id(&req.run_id)?, agent_id, result)
            .await
            .map_err(|e| e.to_string())?;
        serde_json::to_string(&run).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Read-only completion preflight for an owned run. Omit result/completion/memory_disposition to discover exact saved criteria, completion_template, memory_disposition_template and blockers. Project-backed executor tasks must explicitly publish/update durable project knowledge or justify not_applicable. Supply a proposed report/disposition to validate it. Does not complete, claim, acknowledge or verify content truth; run_complete repeats validation atomically."
    )]
    async fn run_completion_check(
        &self,
        Parameters(req): Parameters<CompleteRunRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        let result = req.result_with_completion()?;
        serde_json::to_string(
            &self
                .store
                .run_completion_check(
                    parse_id(&req.run_id)?,
                    authenticated_agent(&parts)?,
                    &result,
                )
                .await
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Lexically search current shared project memory for any readable task using ripgrep over a derived read-only projection, then return authoritative current Morrows MemoryEntry records. Ranking prefers exact phrase matches, then number of matched query terms, match count and recency. Milestones are task execution history and are read through task_get/task_context, not this memory search."
    )]
    async fn memory_search(
        &self,
        Parameters(req): Parameters<MemorySearchRequest>,
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
        let project_id = task
            .project_id
            .ok_or_else(|| "task has no project to search".to_string())?;
        let search = self.memory_search.as_ref().ok_or_else(|| {
            "ripgrep memory search is not configured on this Morrows server".to_string()
        })?;
        let value = search
            .search_project(&self.store, project_id, &req.query, req.top_k)
            .await?;
        serde_json::to_string(&value).map_err(|e| e.to_string())
    }

    #[tool(
        description = "Publish durable knowledge directly to the source task's project. Requires task ownership, assignment history or an open Task Session. Project/author are server-bound; context revision and evidence references are checked. Preserve verification limits in basis/content. Revisions retain the old entry; stale supersession is rejected. Retry with the same idempotency_key and payload."
    )]
    async fn project_memory_publish(
        &self,
        Parameters(req): Parameters<morrows_core::PublishProjectMemory>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        serde_json::to_string(
            &self
                .store
                .publish_project_memory(authenticated_agent(&parts)?, req)
                .await
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Request a role on an existing task without creating a duplicate task or claiming it. Persists a pending request visible in the control-plane queue; approval creates/reuses an assignment but does not start a Run. Exact pending retries reuse the request. Query assignment_request_list for resolution; execution still requires control-plane Run creation."
    )]
    async fn task_request_assignment(
        &self,
        Parameters(req): Parameters<AssignmentRequestInput>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        serde_json::to_string(
            &self
                .store
                .request_assignment(
                    parse_id(&req.task_id)?,
                    authenticated_agent(&parts)?,
                    &req.role,
                    &req.reason,
                )
                .await
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Page the caller's assignment requests, optionally filtered by task. Defaults to pending; include_resolved returns approvals, rejection reasons, withdrawn history and assignment IDs. Read-only; no automatic dispatch or acknowledgement."
    )]
    async fn assignment_request_list(
        &self,
        Parameters(req): Parameters<AssignmentRequestList>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        serde_json::to_string(
            &self
                .store
                .assignment_requests_page(
                    req.task_id.as_deref().map(parse_id).transpose()?,
                    Some(authenticated_agent(&parts)?),
                    req.include_resolved,
                    req.page.limit,
                    req.page.offset,
                )
                .await
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }

    #[tool(
        description = "Withdraw the caller's own pending assignment request with a reason. Preserves history, does not release assignments, and cannot undo an approval or withdraw another Agent's request."
    )]
    async fn assignment_request_withdraw(
        &self,
        Parameters(req): Parameters<WithdrawAssignmentRequest>,
        Extension(parts): Extension<Parts>,
    ) -> Result<String, String> {
        serde_json::to_string(
            &self
                .store
                .resolve_assignment_request(
                    parse_id(&req.request_id)?,
                    Some(authenticated_agent(&parts)?),
                    "withdraw",
                    &req.reason,
                    900,
                )
                .await
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
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
        description = "Patch shared task working memory with a new immutable revision. Omitted fields are preserved; constraint objects merge recursively. Supply expected_context_revision_id to reject stale updates. Requires task ownership, assignment history, or an open Task Session; this does not update project memory."
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
            .update_context_revision(
                task_id,
                UpdateContextRevision {
                    goal: req.goal,
                    background: req.background,
                    constraints: req.constraints,
                    current_summary: req.current_summary,
                    expected_context_revision_id: req
                        .expected_context_revision_id
                        .as_deref()
                        .map(parse_id)
                        .transpose()?,
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
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            format!(
                "{}\n\n{}\n\n{}",
                include_str!("mcp_instructions.md"),
                include_str!("context_capture_instructions.md"),
                include_str!("execution_workflow_instructions.md")
            ),
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
            "memory_search",
            "memory_revise",
            "instructions_get",
            "artifact_create",
            "decision_create",
            "thread_create",
            "message_create",
            "run_milestone",
            "handoff_create",
            "handoff_get",
            "handoff_accept",
            "task_collaboration",
            "assignment_renew",
            "run_milestone",
            "run_checkpoint",
            "run_complete",
            "run_completion_check",
            "project_memory_publish",
            "task_request_assignment",
            "assignment_request_list",
            "assignment_request_withdraw",
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
    async fn memory_search_is_task_readable_and_returns_authoritative_entries() {
        use morrows_core::{CreateMemoryEntry, CreateProject};
        use std::{fs as stdfs, path::PathBuf};

        let base = std::env::temp_dir().join(format!("morrows-mcp-grep-{}", Id::new_v4()));
        stdfs::create_dir_all(&base).unwrap();
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Search".into(),
                description: "Lexical".into(),
            })
            .await
            .unwrap();
        let task = store
            .create_task(serde_json::from_value(json!({
                "project_id":project.id,"title":"Reader","description":"find alpha protocol","state":"ready"
            })).unwrap())
            .await
            .unwrap();
        let owner = store.register_agent("owner", &[]).await.unwrap();
        let reader = store.register_agent("reader", &[]).await.unwrap();
        store
            .claim_task(task.id, owner.id, "executor", 300)
            .await
            .unwrap();
        let memory = store
            .create_memory_entry(CreateMemoryEntry {
                scope_type: "project".into(),
                project_id: Some(project.id),
                agent_instance_id: None,
                task_id: None,
                title: "Protocol ALPHA".into(),
                content: json!({"sentinel":"SEARCH-ALPHA-17"}),
                source_kind: "test".into(),
                source_ref: None,
                visibility: "shared".into(),
                supersedes_memory_id: None,
            })
            .await
            .unwrap();
        let search = MemorySearch::for_test(PathBuf::from("rg"), base.join("projection"));
        let mcp = MorrowsMcp::new_with_memory_search(store.clone(), Some(search));
        let result: Value = serde_json::from_str(
            &mcp.memory_search(
                Parameters(MemorySearchRequest {
                    task_id: task.id.to_string(),
                    query: "alpha protocol".into(),
                    top_k: 5,
                }),
                Extension(parts(Some(reader.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(result["engine"], "ripgrep");
        assert_eq!(result["results"][0]["memory"]["id"], json!(memory.id));
        assert_eq!(
            result["results"][0]["memory"]["content"]["sentinel"],
            "SEARCH-ALPHA-17"
        );
        assert_eq!(result["index_role"], "none");
        let _ = stdfs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn task_context_falls_back_when_ripgrep_is_unavailable() {
        use morrows_core::{CreateMemoryEntry, CreateProject};
        use std::{fs as stdfs, path::PathBuf};

        let base = std::env::temp_dir().join(format!("morrows-mcp-grep-fail-{}", Id::new_v4()));
        stdfs::create_dir_all(&base).unwrap();
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let project = store
            .create_project(CreateProject {
                name: "Fallback".into(),
                description: "Fallback".into(),
            })
            .await
            .unwrap();
        let task = store.create_task(serde_json::from_value(json!({
            "project_id":project.id,"title":"Fallback","description":"keep canonical","state":"ready"
        })).unwrap()).await.unwrap();
        store.create_context_revision(task.id, serde_json::from_value(json!({
            "goal":"read memory","background":"fallback","current_summary":"need memory","constraints":{}
        })).unwrap()).await.unwrap();
        let reader = store.register_agent("reader", &[]).await.unwrap();
        store
            .create_memory_entry(CreateMemoryEntry {
                scope_type: "project".into(),
                project_id: Some(project.id),
                agent_instance_id: None,
                task_id: None,
                title: "Canonical".into(),
                content: json!({"sentinel":"CANONICAL-SURVIVES"}),
                source_kind: "test".into(),
                source_ref: None,
                visibility: "shared".into(),
                supersedes_memory_id: None,
            })
            .await
            .unwrap();
        let search = MemorySearch::for_test(
            PathBuf::from("/definitely/missing/rg"),
            base.join("projection"),
        );
        let mcp = MorrowsMcp::new_with_memory_search(store.clone(), Some(search));
        let context: Value = serde_json::from_str(
            &mcp.task_context(
                Parameters(TaskIdRequest {
                    task_id: task.id.to_string(),
                }),
                Extension(parts(Some(reader.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            context["memory"]["items"][0]["content"]["sentinel"],
            "CANONICAL-SURVIVES"
        );
        assert_eq!(context["memory_retrieval"]["available"], false);
        assert_eq!(context["memory_retrieval"]["engine"], "ripgrep");
        assert_eq!(
            context["memory_retrieval"]["fallback"],
            "task_context.memory"
        );
        let _ = stdfs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn e2e_agent_a_milestone_completion_and_agent_b_exact_memory_continuation() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent_a = store.register_agent("memory-writer-a", &[]).await.unwrap();
        let agent_b = store.register_agent("memory-reader-b", &[]).await.unwrap();
        let project = store
            .create_project(
                serde_json::from_value(json!({
                    "name":"E2E durable memory",
                    "description":"Verify exact cross-agent continuation without chat history"
                }))
                .unwrap(),
            )
            .await
            .unwrap();

        // Exercise the same Git-authoritative backend used in production.
        let initial_head = store.mirror_project_memory(project.id).await.unwrap();
        store
            .cutover_project_memory(project.id, &initial_head)
            .await
            .unwrap();

        let task_a = store
            .create_task(
                serde_json::from_value(json!({
                    "project_id":project.id,
                    "title":"Discover protocol",
                    "description":"Agent A discovers and persists an exact reusable result",
                    "state":"ready"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let context_a = store
            .create_context_revision(
                task_a.id,
                serde_json::from_value(json!({
                    "goal":"Find the exact recovery protocol",
                    "background":"No Session/chat history is created in this E2E",
                    "current_summary":"Investigating sentinel protocol",
                    "constraints":{},
                    "created_by_actor_id":"human:e2e"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let assignment_a = store
            .claim_task(task_a.id, agent_a.id, "executor", 300)
            .await
            .unwrap();
        let run_a = store
            .start_run(assignment_a.id, agent_a.id, None)
            .await
            .unwrap();
        let mcp = MorrowsMcp::new(store.clone());

        let exact_plan = vec![
            "Open /workspace/protocol.md and confirm marker ALPHA-17".to_string(),
            "Run probe --mode safe and require result=PASS".to_string(),
            "Only then continue with protocol-v2".to_string(),
        ];
        let artifact: Value = serde_json::from_str(
            &mcp.artifact_create(
                Parameters(CreateArtifactRequest {
                    task_id: task_a.id.to_string(),
                    input: CreateArtifact {
                        title: "ALPHA-17 probe receipt".into(),
                        uri: "file:///workspace/evidence/alpha-17.json".into(),
                        kind: "test_result".into(),
                        description: "result=PASS; protocol=protocol-v2".into(),
                    },
                }),
                Extension(parts(Some(agent_a.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        let artifact_id = artifact["id"].as_str().unwrap().to_string();
        let milestone: Value = serde_json::from_str(
            &mcp.run_milestone(
                Parameters(MilestoneRunRequest {
                    run_id: run_a.id.to_string(),
                    input: morrows_core::CreateRunMilestone {
                        kind: "milestone".into(),
                        summary: "ALPHA-17 probe passed; protocol-v2 is the verified continuation"
                            .into(),
                        completed: vec!["identified protocol-v2".into()],
                        verified: vec!["probe ALPHA-17 returned PASS".into()],
                        remaining: vec!["future tasks should follow the ordered plan".into()],
                        blockers: vec![],
                        next_step: exact_plan[0].clone(),
                        next_plan: exact_plan.clone(),
                        execution_locations: vec![
                            "/workspace/protocol.md".into(),
                            "probe:ALPHA-17".into(),
                        ],
                        artifact_ids: vec![artifact_id.clone()],
                        decision_ids: vec![],
                    },
                }),
                Extension(parts(Some(agent_a.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(milestone["next_plan"], json!(exact_plan));

        let task_b = store
            .create_task(
                serde_json::from_value(json!({
                    "project_id":project.id,
                    "title":"Continue protocol",
                    "description":"Agent B must recover the exact prior reusable result from Morrows",
                    "state":"ready"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .create_context_revision(
                task_b.id,
                serde_json::from_value(json!({
                    "goal":"Continue from project knowledge",
                    "background":"Do not consult Agent A chat; use Morrows durable state only",
                    "current_summary":"Need prior protocol result",
                    "constraints":{},
                    "created_by_actor_id":"human:e2e"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let assignment_b = store
            .claim_task(task_b.id, agent_b.id, "executor", 300)
            .await
            .unwrap();
        let run_b = store
            .start_run(assignment_b.id, agent_b.id, None)
            .await
            .unwrap();

        // Negative control: a milestone alone is not promoted to project memory.
        let before: Value = serde_json::from_str(
            &mcp.task_context(
                Parameters(TaskIdRequest {
                    task_id: task_b.id.to_string(),
                }),
                Extension(parts(Some(agent_b.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert!(before["memory"]["items"].as_array().unwrap().is_empty());

        let memory_payload = json!({
            "sentinel":"MORROWS-E2E-ALPHA-17",
            "decision":"protocol-v2",
            "verified_fact":"probe ALPHA-17 returned PASS",
            "next_step":exact_plan[0],
            "next_plan":exact_plan,
            "source_milestone_id":milestone["id"],
            "execution_locations":["/workspace/protocol.md","probe:ALPHA-17"]
        });
        let head = store
            .project_memory_head(project.id)
            .await
            .unwrap()
            .unwrap();
        let published: Value = serde_json::from_str(
            &mcp.project_memory_publish(
                Parameters(morrows_core::PublishProjectMemory {
                    task_id: task_a.id,
                    idempotency_key: "e2e-alpha-17".into(),
                    new_memory_id: None,
                    expected_project_id: Some(project.id),
                    base_commit: Some(head),
                    title: "Verified ALPHA-17 continuation protocol".into(),
                    content: memory_payload.clone(),
                    verification_status: "verified".into(),
                    basis: "Agent A milestone recorded the exact probe result and ordered recovery plan."
                        .into(),
                    context_revision_id: context_a.id,
                    artifact_ids: vec![artifact_id.clone()],
                    decision_ids: vec![],
                    supersedes_memory_id: None,
                }),
                Extension(parts(Some(agent_a.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        let memory_id = published["id"].as_str().unwrap().to_string();

        mcp.run_complete(
            Parameters(CompleteRunRequest {
                run_id: run_a.id.to_string(),
                result: json!({
                    "ok":true,
                    "summary":"protocol-v2 recorded and project memory published"
                }),
                completion: None,
                memory_disposition: Some(morrows_core::MemoryDisposition {
                    status: "published".into(),
                    rationale:
                        "The verified protocol and ordered recovery plan are reusable by later project tasks."
                            .into(),
                    memory_entry_ids: vec![memory_id.clone()],
                }),
            }),
            Extension(parts(Some(agent_a.id))),
        )
        .await
        .unwrap();

        // Agent B reads only through the normal task_context surface.
        let after: Value = serde_json::from_str(
            &mcp.task_context(
                Parameters(TaskIdRequest {
                    task_id: task_b.id.to_string(),
                }),
                Extension(parts(Some(agent_b.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        let items = after["memory"]["items"].as_array().unwrap();
        let recovered = items
            .iter()
            .find(|entry| entry["id"] == memory_id)
            .expect("Agent B should receive Agent A's project memory");
        assert_eq!(recovered["content"], memory_payload);
        assert_eq!(recovered["content"]["sentinel"], "MORROWS-E2E-ALPHA-17");
        assert_eq!(recovered["content"]["next_plan"], json!(exact_plan));
        assert_eq!(
            after["project"]["memory_head"],
            json!(store.project_memory_head(project.id).await.unwrap())
        );

        // Agent B actually continues from the recovered durable plan, rather than
        // merely observing that memory existed.
        let recovered_plan: Vec<String> =
            serde_json::from_value(recovered["content"]["next_plan"].clone()).unwrap();
        let recovered_next = recovered["content"]["next_step"]
            .as_str()
            .unwrap()
            .to_string();
        let continued: Value = serde_json::from_str(
            &mcp.run_milestone(
                Parameters(MilestoneRunRequest {
                    run_id: run_b.id.to_string(),
                    input: morrows_core::CreateRunMilestone {
                        kind: "milestone".into(),
                        summary: "Agent B resumed from Agent A's durable project memory".into(),
                        completed: vec![],
                        verified: vec!["recovered MORROWS-E2E-ALPHA-17 exactly".into()],
                        remaining: vec!["execute recovered protocol-v2 plan".into()],
                        blockers: vec![],
                        next_step: recovered_next.clone(),
                        next_plan: recovered_plan.clone(),
                        execution_locations: vec![
                            "/workspace/protocol.md".into(),
                            "probe:ALPHA-17".into(),
                        ],
                        artifact_ids: vec![],
                        decision_ids: vec![],
                    },
                }),
                Extension(parts(Some(agent_b.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        assert_eq!(continued["next_step"], recovered_next);
        assert_eq!(continued["next_plan"], json!(recovered_plan));

        // No Session was used: cross-agent continuation came from milestone/project memory,
        // not copied conversation history.
        assert!(store.list_sessions(None).await.unwrap().is_empty());
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
        let milestone: Value = serde_json::from_str(
            &mcp.run_milestone(
                Parameters(MilestoneRunRequest {
                    run_id: run.id.to_string(),
                    input: morrows_core::CreateRunMilestone {
                        kind: "handoff_preparation".into(),
                        summary: "patch ready for continuation".into(),
                        completed: vec!["patch".into()],
                        verified: vec!["artifact persisted".into()],
                        remaining: vec!["test".into()],
                        blockers: vec![],
                        next_step: "run tests".into(),
                        next_plan: vec![
                            "run tests".into(),
                            "record result before further edits".into(),
                        ],
                        execution_locations: vec!["file:///patch".into()],
                        artifact_ids: vec![artifact["id"].as_str().unwrap().into()],
                        decision_ids: vec![],
                    },
                }),
                Extension(parts(Some(a.id))),
            )
            .await
            .unwrap(),
        )
        .unwrap();
        let request = || {
            Parameters(CreateHandoffRequest {
                run_id: run.id.to_string(),
                input: CreateHandoff {
                    milestone_id: Some(milestone["id"].as_str().unwrap().into()),
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
