import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent, PointerEvent as ReactPointerEvent } from "react";
import "./App.css";
import SessionChat from "./SessionChat";
import AssignmentRequests from "./AssignmentRequests";
import { api } from "./api";
import OperatorLogin from "./OperatorLogin";
import {
  formatAge,
  formatDateTime,
  formatDispatchReason,
  formatOutcome,
  formatRole,
  formatState,
  initialLocale,
  translate,
} from "./i18n";
import type { Locale, TranslationKey } from "./i18n";

type Project = {
  id: string;
  name: string;
  description: string;
  status: string;
  created_at: string;
  updated_at: string;
};

type MemoryEntry = {
  id: string;
  scope_type: string;
  project_id?: string | null;
  agent_instance_id?: string | null;
  task_id?: string | null;
  title: string;
  content: unknown;
  source_kind: string;
  source_ref?: string | null;
  visibility: string;
  supersedes_memory_id?: string | null;
  provenance?: Record<string, unknown> | null;
  created_at: string;
  updated_at: string;
};

type Task = {
  id: string;
  project_id?: string | null;
  title: string;
  description: string;
  owner_actor_id: string;
  state: string;
  assignment_mode: "open" | "approval" | "dispatch";
  priority: number;
  current_context_revision_id?: string | null;
  created_at: string;
  updated_at: string;
};

type TaskManagementSummary = {
  task: Task;
  project_name?: string | null;
  executor_assignment_id?: string | null;
  executor_assignment_status?: string | null;
  executor_agent_instance_id?: string | null;
  executor_agent_display_name?: string | null;
  executor_acquired_at?: string | null;
  latest_run_id?: string | null;
  latest_run_status?: string | null;
  latest_run_started_at?: string | null;
};

type TaskManagementFilter = "all" | "unclaimed" | "assigned" | "running" | "review" | "blocked" | "done" | "cancelled";
type TaskManagementSortKey = "status" | "task" | "project" | "publisher" | "assignee" | "created" | "updated" | "priority";
type TaskManagementSortDirection = "asc" | "desc";
type TaskManagementMode = "tasks" | "chains" | "groups";

type EdgeCondition =
  | { kind: "unconditional" }
  | { kind: "result_equals"; path: string; value: unknown };

type TaskGate = {
  task_id: string;
  join_mode: "all" | "any";
  state: "eligible" | "waiting" | "excluded";
  predecessors: {
    predecessor_id: string;
    condition: EdgeCondition;
    state: "eligible" | "waiting" | "excluded";
    reason: string;
  }[];
  successors: string[];
};

type TaskCollection = {
  id: string;
  name: string;
  description: string;
  archived: boolean;
  created_at: string;
  updated_at: string;
};

type TaskCollectionDetail = {
  collection: TaskCollection;
  members: Task[];
  gates: TaskGate[];
  edges: { task_id: string; depends_on_task_id: string; condition: EdgeCondition }[];
};

type BatchAssignmentResult = {
  task_id: string;
  status: "assigned" | "blocked" | "failed" | string;
  assignment?: Assignment | null;
  gate: TaskGate;
  error?: string | null;
};

type Collaboration = {
  handoffs: {
    id: string;
    summary: string;
    completed: string[];
    remaining: string[];
    blockers: string[];
    context_revision_id: string;
    status: string;
    accepted_by_run_id?: string | null;
  }[];
  artifacts: { id: string; title: string; uri: string; kind: string; description: string }[];
  decisions: { id: string; title: string; rationale: string }[];
  threads: { id: string; title: string }[];
  messages: {
    id: string;
    thread_id: string;
    body: string;
    created_by: string;
    message_type: string;
    recipient_agent_instance_id?: string | null;
    recipient_role?: string | null;
    reply_to_message_id?: string | null;
    correlation_id?: string | null;
    requires_response: boolean;
    status: string;
  }[];
  dependencies: { depends_on_task_id: string }[];
  relationships: {
    source_task_id: string;
    target_task_id: string;
    relation_type: string;
    created_by_actor_id: string;
    metadata: Record<string, unknown>;
    created_at: string;
  }[];
};

type Agent = {
  id: string;
  name: string;
  display_name: string;
  status: string;
  capabilities: string[];
  last_heartbeat_at: string;
  external_instance_ref?: string | null;
  archived_at?: string | null;
};

type FleetEntry = {
  instance: Agent;
  profile: { id: string; name: string; provider: string; kind: string };
  account: {
    id: string;
    label: string;
    email?: string | null;
    provider: string;
    credential_kind?: string | null;
    credential_ref?: string | null;
    status: string;
  } | null;
  machine: { id: string; name: string; hostname: string; os: string; arch: string } | null;
  latest_capacity: {
    status: string;
    available_slots: number;
    active_assignments: number;
    active_runs: number;
    max_concurrency: number | null;
    quota_state: string | null;
    observed_at: string;
  } | null;
  reported_identity: {
    agent_name?: string | null;
    account_email?: string | null;
    platform?: string | null;
    device?: string | null;
    reported_at: string;
  } | null;
};

type Assignment = {
  id: string;
  task_id: string;
  role: string;
  agent_instance_id: string;
  status: string;
  phase: string;
  acquired_at: string;
  expires_at: string;
};

type Run = {
  id: string;
  task_id: string;
  assignment_id: string;
  agent_instance_id: string;
  external_session_ref?: string | null;
  status: string;
  stop_reason?: string | null;
  failure_reason?: string | null;
  checkpoint?: unknown;
  result?: unknown;
  context_revision_id?: string | null;
  started_at: string;
  ended_at?: string | null;
};

type RunExecution = {
  binding: {
    logical_session_id: string;
    restart_deadline_at?: string | null;
  } | null;
  observation: {
    error?: string;
    session?: { status: string; machines_touched: string[]; in_flight_calls: number };
    jobs?: { jobs: { job_id: string; name: string; status: string; machine: string; exit_code?: number | null }[]; complete: boolean; unreachable_machines: string[] };
    shells?: { shells: { session_id: string; machine: string; backend?: string }[]; complete: boolean; unreachable_machines: string[] };
    audit?: { entries: { id: string; tool?: string; event: string; status?: string; ts: number; input?: unknown; output?: unknown }[] };
  } | null;
  evidence?: { id: string; event_id?: string | null; kind: string; reference: string; start_seq?: number | null; end_seq?: number | null }[];
};

type ContextRevision = {
  id: string;
  task_id: string;
  version: number;
  goal: string;
  background: string;
  constraints: unknown;
  current_summary: string;
  created_by_actor_id: string;
  created_at: string;
};

type ContextPackage = {
  id: string;
  work_item_id: string;
  objective: string;
  summary?: unknown | null;
  context_snapshot_id?: string | null;
  memory_refs: string[];
  decision_refs: string[];
  artifact_refs: string[];
  changed_files: string[];
  verified_results: string[];
  blockers: string[];
  unresolved_questions: string[];
  next_action: string;
  source_run_id?: string | null;
  source_agent_id?: string | null;
  created_at: string;
};

type SystemHealth = {
  ok: boolean;
  service: string;
  mcp?: { ready: boolean; path: string };
};

type EventItem = {
  id: string;
  actor_type: string;
  actor_id: string;
  event_type: string;
  payload: unknown;
  created_at: string;
};

type DispatchPolicy = {
  task_id: string;
  role: string;
  required_capabilities: string[];
  profile_id?: string | null;
  account_id?: string | null;
  machine_id?: string | null;
  heartbeat_ttl_seconds: number;
  capacity_ttl_seconds: number;
  lease_seconds: number;
  enabled: boolean;
  created_at: string;
  updated_at: string;
};

type DispatchCandidate = {
  agent_instance_id: string;
  agent_name: string;
  eligible: boolean;
  effective_slots: number;
  current_active_assignments: number;
  heartbeat_age_seconds: number;
  capacity_age_seconds?: number | null;
  capacity_status?: string | null;
  quota_state?: string | null;
  reasons: string[];
};

type DispatchPreview = {
  task_id: string;
  role: string;
  policy: DispatchPolicy;
  task_dispatchable: boolean;
  task_reasons: string[];
  candidates: DispatchCandidate[];
  selected_agent_instance_id?: string | null;
};

type DispatchDecision = {
  id: string;
  task_id: string;
  role: string;
  outcome: string;
  selected_agent_instance_id?: string | null;
  assignment_id?: string | null;
  preview: DispatchPreview;
  created_at: string;
};

type LaunchProfile = {
  id: string;
  name: string;
  adapter: string;
  agent_instance_id: string;
  program: string;
  default_cwd?: string | null;
  model?: string | null;
  enabled: boolean;
  metadata: unknown;
  created_at: string;
  updated_at: string;
};

type ManagedCodexProvision = {
  provider: string;
  account: {
    id: string;
    provider: string;
    label: string;
    email?: string | null;
    credential_kind?: string | null;
    credential_ref?: string | null;
    status: string;
  };
  instance: Agent;
  launch_profile: LaunchProfile;
  credential_kind: string;
  credential_ref: string;
  credential_secret_stored: boolean;
};

type LaunchAttempt = {
  id: string;
  assignment_id: string;
  task_id: string;
  agent_instance_id: string;
  launch_profile_id: string;
  run_id?: string | null;
  session_id?: string | null;
  job_id?: string | null;
  resume_from_attempt_id?: string | null;
  status: string;
  cwd?: string | null;
  external_session_ref?: string | null;
  pid?: number | null;
  exit_code?: number | null;
  stdout_path?: string | null;
  stderr_path?: string | null;
  error?: string | null;
  created_at: string;
  started_at?: string | null;
  ended_at?: string | null;
};

type TaskSession = {
  id: string;
  agent_instance_id: string;
  agent_name: string;
  project_id?: string | null;
  project_name?: string | null;
  task_id?: string | null;
  task_title?: string | null;
  title: string;
  status: string;
  message_count: number;
  queued_count: number;
  undelivered_count: number;
  last_message_preview?: string | null;
  updated_at: string;
};

type AssignmentIntakeView = {
  assignment: Assignment;
  intake: {
    assignment_id: string;
    interview_session_id?: string | null;
    conversation_state: string;
    interview_started_at?: string | null;
    final_summary_message_id?: string | null;
    confirmation_message_id?: string | null;
    converged_at?: string | null;
  };
  blockers: string[];
  execution_ready: boolean;
};

type LaunchInstruction = {
  id: string;
  launch_attempt_id: string;
  actor_type: string;
  actor_id: string;
  body: string;
  created_at: string;
};

function shortId(id: string) {
  return id.slice(0, 8);
}

type SidebarIconName = "project" | "task" | "session" | "agent" | "collapse" | "expand";

function SidebarIcon({ name }: { name: SidebarIconName }) {
  const common = {
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.8,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
  };

  if (name === "project") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" {...common}>
        <path d="M3.5 7.25h6l1.7 2h9.3v8.25a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2V7.25Z" />
        <path d="M3.5 7.25V5.8a1.8 1.8 0 0 1 1.8-1.8h4.05l1.7 2h5.55" />
      </svg>
    );
  }

  if (name === "task") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" {...common}>
        <rect x="4" y="3.5" width="16" height="17" rx="2.5" />
        <path d="M8 8h8M8 12h5.5M8 16h7" />
        <path d="m6.75 8 .65.65 1.1-1.35M6.75 12l.65.65 1.1-1.35M6.75 16l.65.65 1.1-1.35" />
      </svg>
    );
  }

  if (name === "session") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" {...common}>
        <path d="M5.25 5.25h13.5a2.5 2.5 0 0 1 2.5 2.5v7a2.5 2.5 0 0 1-2.5 2.5H11l-4.75 3v-3h-1a2.5 2.5 0 0 1-2.5-2.5v-7a2.5 2.5 0 0 1 2.5-2.5Z" />
        <path d="M7.5 10.25h9M7.5 13.25h5.5" />
      </svg>
    );
  }

  if (name === "agent") {
    return (
      <svg viewBox="0 0 24 24" aria-hidden="true" {...common}>
        <path d="M12 4V2.5M8 5.5h8a3 3 0 0 1 3 3v7a3 3 0 0 1-3 3H8a3 3 0 0 1-3-3v-7a3 3 0 0 1 3-3Z" />
        <path d="M2.75 10.25v4M21.25 10.25v4M8.5 18.5v2M15.5 18.5v2" />
        <circle cx="9.25" cy="11.25" r=".8" fill="currentColor" stroke="none" />
        <circle cx="14.75" cy="11.25" r=".8" fill="currentColor" stroke="none" />
        <path d="M9 14.5h6" />
      </svg>
    );
  }

  return (
    <svg viewBox="0 0 24 24" aria-hidden="true" {...common}>
      <rect x="3.5" y="4" width="17" height="16" rx="2.5" />
      <path d="M8.5 4v16" />
      {name === "collapse" ? <path d="m14.5 9-3 3 3 3" /> : <path d="m11.5 9 3 3-3 3" />}
    </svg>
  );
}

function MorrowsMark() {
  return (
    <svg className="brand-mark" viewBox="0 0 36 36" aria-hidden="true">
      <defs>
        <linearGradient id="morrows-mark-bg" x1="5" y1="4" x2="31" y2="32" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#F4B54A" />
          <stop offset=".55" stopColor="#E77C4F" />
          <stop offset="1" stopColor="#B95C7A" />
        </linearGradient>
        <linearGradient id="morrows-mark-glow" x1="18" y1="9" x2="18" y2="28" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="#FFF8E7" />
          <stop offset="1" stopColor="#F7DFC0" />
        </linearGradient>
      </defs>
      <rect x="1" y="1" width="34" height="34" rx="10" fill="url(#morrows-mark-bg)" />
      <path d="M8.25 24.75V12.5l5.05 7.25L18 12.5l4.7 7.25 5.05-7.25v12.25" stroke="url(#morrows-mark-glow)" strokeWidth="2.35" strokeLinecap="round" strokeLinejoin="round" fill="none" />
      <path d="M8.25 27h19.5" stroke="#5B2D37" strokeOpacity=".32" strokeWidth="1.4" strokeLinecap="round" />
      <circle cx="18" cy="8.5" r="1.45" fill="#FFF4D6" />
    </svg>
  );
}

function StateBadge({ state, locale }: { state: string; locale: Locale }) {
  return <span className={`badge state-${state}`}>{formatState(locale, state)}</span>;
}

function taskManagementStatus(row: TaskManagementSummary): TaskManagementFilter {
  if (row.task.state === "done") return "done";
  if (row.task.state === "cancelled") return "cancelled";
  if (row.task.state === "blocked") return "blocked";
  if (row.task.state === "review") return "review";
  if (
    row.task.state === "in_progress"
    || ["running", "starting", "awaiting_agent", "interrupted", "cleanup_pending", "cancelling"].includes(row.latest_run_status || "")
  ) {
    return "running";
  }
  if (row.executor_assignment_status === "active") return "assigned";
  return "unclaimed";
}

function taskManagementStatusLabel(locale: Locale, status: TaskManagementFilter) {
  const zh: Record<TaskManagementFilter, string> = {
    all: "全部",
    unclaimed: "未接单",
    assigned: "已接单",
    running: "执行中",
    review: "待复核",
    blocked: "阻塞",
    done: "已完成",
    cancelled: "已取消",
  };
  const en: Record<TaskManagementFilter, string> = {
    all: "All",
    unclaimed: "Unclaimed",
    assigned: "Assigned",
    running: "Running",
    review: "Review",
    blocked: "Blocked",
    done: "Completed",
    cancelled: "Cancelled",
  };
  return (locale === "zh-CN" ? zh : en)[status];
}

function agentKindDisplayName(kind: string, locale: Locale) {
  const key = kind.trim().toLowerCase();
  if (key === "coding_agent") return locale === "zh-CN" ? "编码 Agent" : "Coding agent";
  if (key === "orchestrator") return locale === "zh-CN" ? "协调 Agent" : "Orchestrator";
  if (key === "validation") return locale === "zh-CN" ? "验证 Agent" : "Validation agent";
  if (key === "legacy") return locale === "zh-CN" ? "历史记录" : "Legacy record";
  return kind.replaceAll("_", " ");
}

function cleanAgentDisplayName(entry: FleetEntry, locale: Locale) {
  const { instance, profile, machine, reported_identity } = entry;
  const displayName = instance.display_name?.trim();
  const reportedName = reported_identity?.agent_name?.trim();
  const generatedOauthName = profile.provider === "lsm" && /^lsm-oauth-client-\d+$/i.test(displayName || "");
  if (displayName && !generatedOauthName) return displayName;
  if (reportedName) return reportedName;
  if (displayName) return displayName;
  if (profile.kind === "legacy" || profile.provider === "legacy") {
    const raw = instance.name.toLowerCase();
    if (raw.startsWith("codex-personal-handoff-")) {
      const suffix = raw.slice("codex-personal-handoff-".length).toUpperCase();
      return locale === "zh-CN" ? `Codex · 交接 ${suffix}` : `Codex · Handoff ${suffix}`;
    }
    if (raw === "codex-personal-macbook") return "Codex · MacBook";
    return instance.name.replaceAll("-", " ");
  }

  const base = profile.name || instance.name;
  const atIndex = instance.name.indexOf("@");
  if (atIndex >= 0) {
    const suffix = instance.name.slice(atIndex + 1).replace(/-session$/i, "");
    if (suffix && suffix !== machine?.name) {
      if (/^m\d+$/i.test(suffix)) return `${base} · ${suffix.toUpperCase()}`;
    }
  }
  return base;
}

function accountDisplayName(entry: FleetEntry, locale: Locale) {
  return entry.reported_identity?.account_email?.trim()
    || (locale === "zh-CN" ? "未上报" : "Not reported");
}

function platformDisplayName(entry: FleetEntry, locale: Locale) {
  return entry.reported_identity?.platform?.trim()
    || (locale === "zh-CN" ? "未上报" : "Not reported");
}

function machineDisplayName(entry: FleetEntry, locale: Locale) {
  return entry.reported_identity?.device?.trim()
    || (locale === "zh-CN" ? "未上报" : "Not reported");
}

function agentAvailabilityRank(entry: FleetEntry) {
  if (entry.instance.status === "online" && entry.latest_capacity?.status === "available") return 0;
  if (entry.instance.status === "online") return 1;
  if (entry.latest_capacity?.status === "busy" || entry.latest_capacity?.status === "throttled") return 2;
  return 3;
}

export default function App() {
  const [locale, setLocale] = useState<Locale>(initialLocale);
  const [theme, setTheme] = useState<"dark" | "light">(() => {
    const saved = window.localStorage.getItem("morrows.theme");
    if (saved === "dark" || saved === "light") return saved;
    return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
  });
  const [fontScale, setFontScale] = useState(() => {
    const saved = Number(window.localStorage.getItem("morrows.fontScale"));
    return Number.isFinite(saved) && saved >= 0.85 && saved <= 1.5 ? saved : 1.15;
  });
  const [showSettings, setShowSettings] = useState(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => window.localStorage.getItem("morrows.sidebarCollapsed") === "1");
  const [projectsPaneWidth, setProjectsPaneWidth] = useState(() => {
    const saved = Number(window.localStorage.getItem("morrows.projectsPaneWidth"));
    return Number.isFinite(saved) && saved >= 260 && saved <= 720 ? saved : 360;
  });
  const [view, setView] = useState<"sessions" | "queue" | "tasks" | "agents">("sessions");
  const [projects, setProjects] = useState<Project[]>([]);
  const [tasks, setTasks] = useState<Task[]>([]);
  const [taskManagement, setTaskManagement] = useState<TaskManagementSummary[]>([]);
  const [taskManagementFilter, setTaskManagementFilter] = useState<TaskManagementFilter>("all");
  const [taskManagementQuery, setTaskManagementQuery] = useState("");
  const [taskManagementMode, setTaskManagementMode] = useState<TaskManagementMode>("tasks");
  const [taskChains, setTaskChains] = useState<TaskCollection[]>([]);
  const [taskGroups, setTaskGroups] = useState<TaskCollection[]>([]);
  const [selectedChainId, setSelectedChainId] = useState<string | null>(null);
  const [selectedGroupId, setSelectedGroupId] = useState<string | null>(null);
  const [chainDetail, setChainDetail] = useState<TaskCollectionDetail | null>(null);
  const [groupDetail, setGroupDetail] = useState<TaskCollectionDetail | null>(null);
  const [newChainName, setNewChainName] = useState("");
  const [newGroupName, setNewGroupName] = useState("");
  const [chainMemberTaskId, setChainMemberTaskId] = useState("");
  const [chainJoinMode, setChainJoinMode] = useState<"all" | "any">("all");
  const [chainEdgePredecessorId, setChainEdgePredecessorId] = useState("");
  const [chainEdgeSuccessorId, setChainEdgeSuccessorId] = useState("");
  const [chainEdgeConditionKind, setChainEdgeConditionKind] = useState<"unconditional" | "result_equals">("unconditional");
  const [chainEdgePath, setChainEdgePath] = useState("/kind");
  const [chainEdgeValue, setChainEdgeValue] = useState('"success"');
  const [groupMemberTaskId, setGroupMemberTaskId] = useState("");
  const [groupAgentId, setGroupAgentId] = useState("");
  const [groupAssignResults, setGroupAssignResults] = useState<BatchAssignmentResult[]>([]);
  const [collectionBusy, setCollectionBusy] = useState(false);
  const [taskGate, setTaskGate] = useState<TaskGate | null>(null);
  const [taskManagementSort, setTaskManagementSort] = useState<{
    key: TaskManagementSortKey;
    direction: TaskManagementSortDirection;
  }>({ key: "created", direction: "desc" });
  const [agents, setAgents] = useState<FleetEntry[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [assignments, setAssignments] = useState<Assignment[]>([]);
  const [runs, setRuns] = useState<Run[]>([]);
  const [openRunId, setOpenRunId] = useState<string | null>(null);
  const [runExecutions, setRunExecutions] = useState<Record<string, RunExecution>>({});
  const [jobOutputs, setJobOutputs] = useState<Record<string, string>>({});
  const [events, setEvents] = useState<EventItem[]>([]);
  const [collaboration, setCollaboration] = useState<Collaboration | null>(null);
  const [context, setContext] = useState<ContextRevision | null>(null);
  const [contextPackage, setContextPackage] = useState<ContextPackage | null>(null);
  const [contextBusy, setContextBusy] = useState(false);
  const [systemHealth, setSystemHealth] = useState<SystemHealth | null>(null);
  const [healthCheckedAt, setHealthCheckedAt] = useState<Date | null>(null);
  const [dispatchPolicies, setDispatchPolicies] = useState<DispatchPolicy[]>([]);
  const [dispatchPolicy, setDispatchPolicy] = useState<DispatchPolicy | null>(null);
  const [dispatchPreview, setDispatchPreview] = useState<DispatchPreview | null>(null);
  const [dispatchDecisions, setDispatchDecisions] = useState<DispatchDecision[]>([]);
  const [launchProfiles, setLaunchProfiles] = useState<LaunchProfile[]>([]);
  const [launchAttempts, setLaunchAttempts] = useState<LaunchAttempt[]>([]);
  const [launchInstructions, setLaunchInstructions] = useState<LaunchInstruction[]>([]);
  const [instructionDrafts, setInstructionDrafts] = useState<Record<string, string>>({});
  const [launchBusy, setLaunchBusy] = useState(false);
  const [dispatchCapabilities, setDispatchCapabilities] = useState("");
  const [dispatchEnabled, setDispatchEnabled] = useState(true);
  const [dispatchLease, setDispatchLease] = useState(900);
  const [dispatchBusy, setDispatchBusy] = useState(false);
  const [title, setTitle] = useState("");
  const [priority, setPriority] = useState(0);
  const [newTaskProjectId, setNewTaskProjectId] = useState("");
  const [projectName, setProjectName] = useState("");
  const [projectSort, setProjectSort] = useState<"updated_desc" | "created_desc" | "name_asc" | "task_count_desc">(() => {
    const saved = window.localStorage.getItem("morrows.projectSort");
    return saved === "created_desc" || saved === "name_asc" || saved === "task_count_desc" ? saved : "updated_desc";
  });
  const [showProjectCreate, setShowProjectCreate] = useState(false);
  const [projectBusy, setProjectBusy] = useState(false);
  const [assignmentModeBusy, setAssignmentModeBusy] = useState(false);
  const [taskMutationBusy, setTaskMutationBusy] = useState(false);
  const [selectedProjectId, setSelectedProjectId] = useState<string | null>(null);
  const [expandedProjectId, setExpandedProjectId] = useState<string | null>(null);
  const [projectMemories, setProjectMemories] = useState<MemoryEntry[]>([]);
  const [selectedProjectMemoryId, setSelectedProjectMemoryId] = useState<string | null>(null);
  const [projectMemoryLoading, setProjectMemoryLoading] = useState(false);
  const [taskSessions, setTaskSessions] = useState<TaskSession[]>([]);
  const [assignmentIntake, setAssignmentIntake] = useState<AssignmentIntakeView | null>(null);
  const [sessionAgentId, setSessionAgentId] = useState<string | null>(null);
  const [sessionTargetId, setSessionTargetId] = useState<string | null>(null);
  const [sessionTaskId, setSessionTaskId] = useState<string | null>(null);
  const [renamingAgentId, setRenamingAgentId] = useState<string | null>(null);
  const [agentNameDraft, setAgentNameDraft] = useState("");
  const [agentRenameBusy, setAgentRenameBusy] = useState(false);
  const [showAgentCreate, setShowAgentCreate] = useState(false);
  const [newAgentEmail, setNewAgentEmail] = useState("");
  const [newAgentCredentialRef, setNewAgentCredentialRef] = useState("~/.codex");
  const [newAgentDisplayName, setNewAgentDisplayName] = useState("");
  const [newAgentModel, setNewAgentModel] = useState("");
  const [agentCreateBusy, setAgentCreateBusy] = useState(false);
  const [agentCreateResult, setAgentCreateResult] = useState<ManagedCodexProvision | null>(null);
  const [error, setError] = useState<string | null>(null);

  const orderedAgents = useMemo(
    () => [...agents].sort((a, b) =>
      agentAvailabilityRank(a) - agentAvailabilityRank(b)
      || cleanAgentDisplayName(a, locale).localeCompare(cleanAgentDisplayName(b, locale), locale)
    ),
    [agents, locale],
  );
  const fleetSummary = useMemo(() => ({
    total: agents.length,
    online: agents.filter((entry) => entry.instance.status === "online").length,
    working: agents.filter((entry) =>
      (entry.latest_capacity?.active_assignments ?? 0) > 0
      || (entry.latest_capacity?.active_runs ?? 0) > 0
      || entry.latest_capacity?.status === "busy"
    ).length,
    available: agents.filter((entry) =>
      entry.instance.status === "online" && (entry.latest_capacity?.available_slots ?? 0) > 0
    ).length,
  }), [agents]);

  const selectedTask = useMemo(() => tasks.find((task) => task.id === selectedId) ?? null, [tasks, selectedId]);
  const selectedProject = useMemo(
    () => projects.find((project) => project.id === selectedTask?.project_id) ?? null,
    [projects, selectedTask?.project_id],
  );
  const openedProject = useMemo(
    () => projects.find((project) => project.id === selectedProjectId) ?? null,
    [projects, selectedProjectId],
  );
  const unclassifiedTasks = useMemo(
    () => tasks.filter((task) => !task.project_id),
    [tasks],
  );
  const unclassifiedOpen = selectedProjectId === "__unclassified__";
  const selectedProjectMemory = useMemo(
    () => projectMemories.find((memory) => memory.id === selectedProjectMemoryId) ?? projectMemories[0] ?? null,
    [projectMemories, selectedProjectMemoryId],
  );
  const projectGroups = useMemo(() => {
    const grouped: { id: string; project: Project | null; tasks: Task[] }[] = projects.map((project) => ({
      id: project.id,
      project,
      tasks: tasks.filter((task) => task.project_id === project.id),
    }));

    grouped.sort((a, b) => {
      if (!a.project || !b.project) return 0;
      if (projectSort === "created_desc") {
        return new Date(b.project.created_at).getTime() - new Date(a.project.created_at).getTime();
      }
      if (projectSort === "name_asc") {
        return a.project.name.localeCompare(b.project.name, locale);
      }
      if (projectSort === "task_count_desc") {
        return b.tasks.length - a.tasks.length || a.project.name.localeCompare(b.project.name, locale);
      }
      const latestUpdate = (group: { project: Project | null; tasks: Task[] }) =>
        Math.max(
          new Date(group.project?.updated_at || 0).getTime(),
          ...group.tasks.map((task) => new Date(task.updated_at).getTime()),
        );
      return latestUpdate(b) - latestUpdate(a);
    });

    const unclassified = tasks.filter((task) => !task.project_id);
    if (unclassified.length) {
      grouped.push({
        id: "__unclassified__",
        project: null as Project | null,
        tasks: unclassified,
      });
    }
    return grouped;
  }, [projects, tasks, projectSort, locale]);

  const taskManagementCounts = useMemo(() => {
    const counts: Record<TaskManagementFilter, number> = {
      all: taskManagement.length,
      unclaimed: 0,
      assigned: 0,
      running: 0,
      review: 0,
      blocked: 0,
      done: 0,
      cancelled: 0,
    };
    taskManagement.forEach((row) => { counts[taskManagementStatus(row)] += 1; });
    return counts;
  }, [taskManagement]);

  const filteredTaskManagement = useMemo(() => {
    const query = taskManagementQuery.trim().toLocaleLowerCase(locale);
    const statusRank: Record<Exclude<TaskManagementFilter, "all">, number> = {
      unclaimed: 0,
      assigned: 1,
      running: 2,
      review: 3,
      blocked: 4,
      done: 5,
      cancelled: 6,
    };
    const rows = taskManagement
      .map((row, sourceIndex) => ({ row, sourceIndex }))
      .filter(({ row }) => {
        const status = taskManagementStatus(row);
        if (taskManagementFilter !== "all" && status !== taskManagementFilter) return false;
        if (!query) return true;
        return [
          row.task.title,
          row.task.description,
          row.project_name || "",
          actorDisplayName(row.task.owner_actor_id),
          row.task.owner_actor_id,
          row.executor_agent_display_name || "",
          row.executor_agent_instance_id || "",
        ].some((value) => value.toLocaleLowerCase(locale).includes(query));
      });

    const sortValue = (row: TaskManagementSummary): string | number | null => {
      switch (taskManagementSort.key) {
        case "status": return statusRank[taskManagementStatus(row) as Exclude<TaskManagementFilter, "all">];
        case "task": return row.task.title;
        case "project": return row.project_name?.trim() || null;
        case "publisher": return actorDisplayName(row.task.owner_actor_id);
        case "assignee": return row.executor_agent_display_name?.trim() || null;
        case "created": return new Date(row.task.created_at).getTime();
        case "updated": return new Date(row.task.updated_at).getTime();
        case "priority": return row.task.priority;
      }
    };
    rows.sort((a, b) => {
      const left = sortValue(a.row);
      const right = sortValue(b.row);
      if (left === null && right === null) return a.sourceIndex - b.sourceIndex;
      if (left === null) return 1;
      if (right === null) return -1;
      const compared = typeof left === "number" && typeof right === "number"
        ? left - right
        : String(left).localeCompare(String(right), locale, { sensitivity: "base", numeric: true });
      if (compared !== 0) return taskManagementSort.direction === "asc" ? compared : -compared;
      return a.sourceIndex - b.sourceIndex;
    });
    return rows.map(({ row }) => row);
  }, [taskManagement, taskManagementFilter, taskManagementQuery, taskManagementSort, locale]);

  const t = (key: TranslationKey) => translate(locale, key);

  useEffect(() => {
    window.localStorage.setItem("morrows.locale", locale);
    document.documentElement.lang = locale;
  }, [locale]);

  useEffect(() => {
    window.localStorage.setItem("morrows.theme", theme);
    document.documentElement.dataset.theme = theme;
  }, [theme]);

  useEffect(() => {
    window.localStorage.setItem("morrows.fontScale", String(fontScale));
    document.documentElement.style.setProperty("--font-scale", String(fontScale));
  }, [fontScale]);

  useEffect(() => {
    window.localStorage.setItem("morrows.sidebarCollapsed", sidebarCollapsed ? "1" : "0");
  }, [sidebarCollapsed]);

  useEffect(() => {
    window.localStorage.setItem("morrows.projectsPaneWidth", String(projectsPaneWidth));
  }, [projectsPaneWidth]);

  useEffect(() => {
    window.localStorage.setItem("morrows.projectSort", projectSort);
  }, [projectSort]);


  const refreshHealth = useCallback(async () => {
    try {
      const health = await api<SystemHealth>("/api/health");
      setSystemHealth(health);
      setHealthCheckedAt(new Date());
    } catch {
      setSystemHealth(null);
      setHealthCheckedAt(new Date());
    }
  }, []);

  const refreshFleet = useCallback(async () => {
    try {
      setAgents(await api<FleetEntry[]>("/api/agent-fleet"));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const refreshSessionScopes = useCallback(async () => {
    try {
      const [nextTasks, nextProjects] = await Promise.all([
        api<Task[]>("/api/tasks"),
        api<Project[]>("/api/projects"),
      ]);
      setTasks(nextTasks);
      setProjects(nextProjects);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const refreshQueueBase = useCallback(async () => {
    try {
      const [nextTasks, nextProjects, nextPolicies, nextLaunchProfiles] = await Promise.all([
        api<Task[]>("/api/tasks"),
        api<Project[]>("/api/projects"),
        api<DispatchPolicy[]>("/api/dispatch-policies"),
        api<LaunchProfile[]>("/api/launch-profiles"),
      ]);
      setTasks(nextTasks);
      setProjects(nextProjects);
      setDispatchPolicies(nextPolicies);
      setLaunchProfiles(nextLaunchProfiles);
      if (!selectedId && nextTasks.length) setSelectedId(nextTasks[0].id);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [selectedId]);

  const refreshTaskManagement = useCallback(async () => {
    try {
      const [next, chains, groups] = await Promise.all([
        api<TaskManagementSummary[]>("/api/tasks/management"),
        api<TaskCollection[]>("/api/task-chains"),
        api<TaskCollection[]>("/api/task-groups"),
      ]);
      setTaskManagement(next);
      setTasks(next.map((row) => row.task));
      setTaskChains(chains);
      setTaskGroups(groups);
      setSelectedChainId((current) => current ?? chains.find((item) => !item.archived)?.id ?? chains[0]?.id ?? null);
      setSelectedGroupId((current) => current ?? groups.find((item) => !item.archived)?.id ?? groups[0]?.id ?? null);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  const refreshDetail = useCallback(async () => {
    if (view !== "queue" || !selectedId || selectedProjectId) return;
    try {
      const [nextAssignments, nextRuns, nextEvents, nextContext, nextContextPackage, nextCollaboration, nextPolicy, nextPreview, nextDispatchDecisions, nextLaunchAttempts, nextLaunchInstructions, nextTaskSessions, nextGate] = await Promise.all([
        api<Assignment[]>(`/api/tasks/${selectedId}/assignments`),
        api<Run[]>(`/api/tasks/${selectedId}/runs`),
        api<EventItem[]>(`/api/tasks/${selectedId}/events`),
        api<ContextRevision>(`/api/tasks/${selectedId}/context`).catch(() => null),
        api<ContextPackage | null>(`/api/tasks/${selectedId}/context-package`).catch(() => null),
        api<Collaboration>(`/api/tasks/${selectedId}/collaboration`),
        api<DispatchPolicy>(`/api/tasks/${selectedId}/dispatch-policy/executor`).catch(() => null),
        api<DispatchPreview>(`/api/tasks/${selectedId}/dispatch-preview/executor`).catch(() => null),
        api<DispatchDecision[]>(`/api/tasks/${selectedId}/dispatch-decisions`),
        api<LaunchAttempt[]>(`/api/tasks/${selectedId}/launch-attempts`),
        api<LaunchInstruction[]>(`/api/tasks/${selectedId}/launch-instructions`),
        api<TaskSession[]>(`/api/sessions?task_id=${encodeURIComponent(selectedId)}`),
        api<TaskGate>(`/api/tasks/${selectedId}/gate`),
      ]);
      setAssignments(nextAssignments);
      setRuns(nextRuns);
      setEvents(nextEvents);
      setContext(nextContext);
      setContextPackage(nextContextPackage);
      setCollaboration(nextCollaboration);
      setDispatchPolicy(nextPolicy);
      setDispatchPreview(nextPreview);
      setDispatchDecisions(nextDispatchDecisions);
      setLaunchAttempts(nextLaunchAttempts);
      setLaunchInstructions(nextLaunchInstructions);
      setTaskSessions(nextTaskSessions);
      setTaskGate(nextGate);
      const intakeAssignment = nextAssignments.find(
        (assignment) => assignment.role === "executor" && assignment.status === "active" && assignment.phase !== "implementing",
      );
      if (intakeAssignment) {
        const nextIntake = await api<AssignmentIntakeView>(`/api/assignments/${intakeAssignment.id}/intake`).catch(() => null);
        setAssignmentIntake(nextIntake);
      } else {
        setAssignmentIntake(null);
      }
      if (openRunId && nextRuns.some((run) => run.id === openRunId)) {
        void api<RunExecution>(`/api/runs/${openRunId}/execution`)
          .then((execution) => setRunExecutions((current) => ({ ...current, [openRunId]: execution })))
          .catch(() => {});
      }
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [view, selectedId, selectedProjectId, openRunId]);

  useEffect(() => {
    void refreshHealth();
    const timer = window.setInterval(() => void refreshHealth(), 5000);
    return () => window.clearInterval(timer);
  }, [refreshHealth]);

  useEffect(() => {
    void refreshFleet();
    const timer = window.setInterval(() => void refreshFleet(), 5000);
    return () => window.clearInterval(timer);
  }, [refreshFleet]);

  useEffect(() => {
    if (view !== "sessions") return;
    void refreshSessionScopes();
    const timer = window.setInterval(() => void refreshSessionScopes(), 5000);
    return () => window.clearInterval(timer);
  }, [view, refreshSessionScopes]);

  useEffect(() => {
    if (view === "queue") {
      void refreshQueueBase();
      const timer = window.setInterval(() => void refreshQueueBase(), 3000);
      return () => window.clearInterval(timer);
    }
    if (view === "tasks") {
      void refreshTaskManagement();
      const timer = window.setInterval(() => void refreshTaskManagement(), 3000);
      return () => window.clearInterval(timer);
    }
    return undefined;
  }, [view, refreshQueueBase, refreshTaskManagement]);

  useEffect(() => {
    void refreshDetail();
    const timer = window.setInterval(() => void refreshDetail(), 3000);
    return () => window.clearInterval(timer);
  }, [refreshDetail]);

  useEffect(() => {
    if (view !== "tasks" || !selectedChainId) return;
    let cancelled = false;
    void api<TaskCollectionDetail>(`/api/task-chains/${selectedChainId}`).then((detail) => {
      if (!cancelled) setChainDetail(detail);
    }).catch((e) => {
      if (!cancelled) setError(e instanceof Error ? e.message : String(e));
    });
    return () => { cancelled = true; };
  }, [view, selectedChainId]);

  useEffect(() => {
    if (view !== "tasks" || !selectedGroupId) return;
    let cancelled = false;
    void api<TaskCollectionDetail>(`/api/task-groups/${selectedGroupId}`).then((detail) => {
      if (!cancelled) setGroupDetail(detail);
    }).catch((e) => {
      if (!cancelled) setError(e instanceof Error ? e.message : String(e));
    });
    return () => { cancelled = true; };
  }, [view, selectedGroupId]);

  useEffect(() => {
    if (view !== "queue" || !selectedId || selectedProjectId) return;
    void api<DispatchPolicy>(`/api/tasks/${selectedId}/dispatch-policy/executor`)
      .then((policy) => {
        setDispatchCapabilities(policy.required_capabilities.join(", "));
        setDispatchEnabled(policy.enabled);
        setDispatchLease(policy.lease_seconds);
      })
      .catch(() => {
        setDispatchCapabilities("");
        setDispatchEnabled(true);
        setDispatchLease(900);
      });
  }, [view, selectedId, selectedProjectId]);

  async function saveContext(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!selectedId) return;
    const form = new FormData(event.currentTarget);
    const goal = String(form.get("goal") || "").trim();
    const background = String(form.get("background") || "").trim();
    const currentSummary = String(form.get("current_summary") || "").trim();
    const constraintsText = String(form.get("constraints") || "{}").trim();
    setContextBusy(true);
    try {
      let constraints: unknown;
      try {
        constraints = JSON.parse(constraintsText || "{}");
      } catch {
        throw new Error(t("invalidConstraintsJson"));
      }
      const next = await api<ContextRevision>(`/api/tasks/${selectedId}/context`, {
        method: "POST",
        body: JSON.stringify({
          goal,
          background,
          current_summary: currentSummary,
          constraints,
          created_by_actor_id: "human:webui",
        }),
      });
      setContext(next);
      await refreshQueueBase();
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setContextBusy(false);
    }
  }

  async function assembleContextPackage() {
    if (!selectedId) return;
    setContextBusy(true);
    try {
      const assembled = await api<ContextPackage>(`/api/tasks/${selectedId}/context-package`, {
        method: "POST",
        body: JSON.stringify({ assemble: true }),
      });
      setContextPackage(assembled);
      await refreshDetail();
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setContextBusy(false);
    }
  }

  async function saveDispatchPolicy() {
    if (!selectedId) return;
    setDispatchBusy(true);
    try {
      const required_capabilities = dispatchCapabilities
        .split(",")
        .map((value) => value.trim())
        .filter(Boolean);
      await api<DispatchPolicy>(`/api/tasks/${selectedId}/dispatch-policy`, {
        method: "POST",
        body: JSON.stringify({
          role: "executor",
          required_capabilities,
          heartbeat_ttl_seconds: 120,
          capacity_ttl_seconds: 120,
          lease_seconds: dispatchLease,
          enabled: dispatchEnabled,
        }),
      });
      await Promise.all([refreshQueueBase(), refreshDetail()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDispatchBusy(false);
    }
  }

  async function previewDispatch() {
    if (!selectedId) return;
    setDispatchBusy(true);
    try {
      setDispatchPreview(await api<DispatchPreview>(`/api/tasks/${selectedId}/dispatch-preview/executor`));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDispatchBusy(false);
    }
  }

  async function dispatchSelectedTask() {
    if (!selectedId) return;
    setDispatchBusy(true);
    try {
      await api(`/api/tasks/${selectedId}/dispatch/executor`, { method: "POST", body: "null" });
      await Promise.all([refreshQueueBase(), refreshDetail()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDispatchBusy(false);
    }
  }

  async function dispatchNextTask() {
    setDispatchBusy(true);
    try {
      await api("/api/dispatch/next/executor", { method: "POST", body: "null" });
      await Promise.all([refreshQueueBase(), refreshDetail()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDispatchBusy(false);
    }
  }

  async function enqueueLaunch(assignmentId: string, launchProfileId: string, resumeFromAttemptId?: string) {
    setLaunchBusy(true);
    try {
      await api<LaunchAttempt>("/api/launch-attempts/enqueue", {
        method: "POST",
        body: JSON.stringify({
          assignment_id: assignmentId,
          launch_profile_id: launchProfileId,
          resume_from_attempt_id: resumeFromAttemptId,
        }),
      });
      await Promise.all([refreshQueueBase(), refreshDetail()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLaunchBusy(false);
    }
  }

  async function sendLaunchInstruction(attemptId: string) {
    const body = instructionDrafts[attemptId]?.trim();
    if (!body) return;
    setLaunchBusy(true);
    try {
      await api(`/api/launch-attempts/${attemptId}/instructions`, {
        method: "POST",
        body: JSON.stringify({ launch_attempt_id: attemptId, body }),
      });
      setInstructionDrafts((current) => ({ ...current, [attemptId]: "" }));
      await refreshDetail();
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLaunchBusy(false);
    }
  }

  async function stopLaunch(attemptId: string) {
    setLaunchBusy(true);
    try {
      await api(`/api/launch-attempts/${attemptId}/stop`, { method: "POST", body: "null" });
      await Promise.all([refreshQueueBase(), refreshDetail()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLaunchBusy(false);
    }
  }

  async function toggleRunExecution(runId: string) {
    if (openRunId === runId) {
      setOpenRunId(null);
      return;
    }
    setOpenRunId(runId);
    try {
      const execution = await api<RunExecution>(`/api/runs/${runId}/execution`);
      setRunExecutions((current) => ({ ...current, [runId]: execution }));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function mutateRun(runId: string, action: "restart" | "cancel") {
    setLaunchBusy(true);
    try {
      await api(`/api/runs/${runId}/${action}`, { method: "POST", body: "null" });
      await Promise.all([refreshQueueBase(), refreshDetail()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLaunchBusy(false);
    }
  }

  async function loadJobOutput(runId: string, jobId: string, machine: string) {
    const key = `${runId}:${jobId}`;
    try {
      const result = await api<{ output: string }>(`/api/runs/${runId}/jobs/${jobId}/output?machine=${encodeURIComponent(machine)}`);
      setJobOutputs((current) => ({ ...current, [key]: result.output || "" }));
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  function selectTask(taskId: string) {
    setSelectedProjectId(null);
    setProjectMemories([]);
    setSelectedProjectMemoryId(null);
    setSelectedId(taskId);
  }

  function openUnclassified() {
    setSelectedProjectId("__unclassified__");
    setProjectMemories([]);
    setSelectedProjectMemoryId(null);
    setProjectMemoryLoading(false);
    setError(null);
  }

  async function openProject(project: Project) {
    setSelectedProjectId(project.id);
    setProjectMemoryLoading(true);
    try {
      const memories = await api<MemoryEntry[]>(
        `/api/memories?scope_type=project&project_id=${encodeURIComponent(project.id)}`,
      );
      setProjectMemories(memories);
      setSelectedProjectMemoryId(memories[0]?.id ?? null);
      setError(null);
    } catch (e) {
      setProjectMemories([]);
      setSelectedProjectMemoryId(null);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setProjectMemoryLoading(false);
    }
  }

  async function renameAgent(agentId: string) {
    const displayName = agentNameDraft.trim();
    if (!displayName) return;
    setAgentRenameBusy(true);
    try {
      await api(`/api/agent-instances/${agentId}/rename`, {
        method: "POST",
        body: JSON.stringify({ display_name: displayName }),
      });
      setRenamingAgentId(null);
      setAgentNameDraft("");
      await refreshFleet();
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setAgentRenameBusy(false);
    }
  }

  async function createManagedCodexAgent(event: FormEvent) {
    event.preventDefault();
    const email = newAgentEmail.trim().toLowerCase();
    const credentialRef = newAgentCredentialRef.trim();
    if (!email || !email.includes("@") || !credentialRef || agentCreateBusy) return;
    setAgentCreateBusy(true);
    try {
      const provisioned = await api<ManagedCodexProvision>("/api/agent-fleet/codex", {
        method: "POST",
        body: JSON.stringify({
          email,
          credential_ref: credentialRef,
          display_name: newAgentDisplayName.trim() || null,
          model: newAgentModel.trim() || null,
        }),
      });
      setAgentCreateResult(provisioned);
      setNewAgentEmail("");
      setNewAgentCredentialRef("~/.codex");
      setNewAgentDisplayName("");
      setNewAgentModel("");
      await Promise.all([refreshFleet(), refreshQueueBase()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setAgentCreateBusy(false);
    }
  }

  function closeAgentCreate() {
    setShowAgentCreate(false);
    setAgentCreateResult(null);
    setNewAgentEmail("");
    setNewAgentCredentialRef("~/.codex");
    setNewAgentDisplayName("");
    setNewAgentModel("");
  }

  async function createTask(event: FormEvent) {
    event.preventDefault();
    if (!title.trim()) return;
    try {
      const created = await api<Task>("/api/tasks", {
        method: "POST",
        body: JSON.stringify({
          project_id: newTaskProjectId || null,
          title: title.trim(),
          description: "",
          priority,
        }),
      });
      setTitle("");
      setPriority(0);
      selectTask(created.id);
      await refreshQueueBase();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  async function createProject(event: FormEvent) {
    event.preventDefault();
    const name = projectName.trim();
    if (!name) return;
    setProjectBusy(true);
    try {
      const created = await api<Project>("/api/projects", {
        method: "POST",
        body: JSON.stringify({ name, description: "" }),
      });
      setProjectName("");
      setShowProjectCreate(false);
      setNewTaskProjectId(created.id);
      await refreshQueueBase();
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setProjectBusy(false);
    }
  }

  async function moveTaskToProject(projectId: string) {
    if (!selectedTask) return;
    setProjectBusy(true);
    try {
      const next = await api<Task>(`/api/tasks/${selectedTask.id}/project`, {
        method: "POST",
        body: JSON.stringify({ project_id: projectId || null }),
      });
      setTasks((current) => current.map((task) => task.id === next.id ? next : task));
      await Promise.all([refreshQueueBase(), refreshDetail()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setProjectBusy(false);
    }
  }

  async function changeTaskAssignmentMode(assignmentMode: Task["assignment_mode"]) {
    if (!selectedTask) return;
    setAssignmentModeBusy(true);
    try {
      const next = await api<Task>(`/api/tasks/${selectedTask.id}/assignment-mode`, {
        method: "POST",
        body: JSON.stringify({ assignment_mode: assignmentMode }),
      });
      setTasks((current) => current.map((task) => task.id === next.id ? next : task));
      await Promise.all([refreshQueueBase(), refreshDetail()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setAssignmentModeBusy(false);
    }
  }

  async function cancelTask(task: Task) {
    const message = t("cancelTaskConfirm").replace("{title}", task.title);
    if (!window.confirm(message)) return;
    setTaskMutationBusy(true);
    try {
      const next = await api<Task>(`/api/tasks/${task.id}/cancel`, { method: "POST" });
      setTasks((current) => current.map((item) => item.id === next.id ? next : item));
      setTaskManagement((current) => current.map((row) => row.task.id === next.id ? { ...row, task: next } : row));
      await Promise.all([
        refreshQueueBase(),
        refreshTaskManagement(),
        ...(selectedId === task.id ? [refreshDetail()] : []),
      ]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setTaskMutationBusy(false);
    }
  }

  async function deleteTask(task: Task) {
    const message = t("deleteTaskConfirm").replace("{title}", task.title);
    if (!window.confirm(message)) return;
    setTaskMutationBusy(true);
    try {
      await api<{ task_id: string; deleted: boolean }>(`/api/tasks/${task.id}`, { method: "DELETE" });
      const nextTaskId = tasks.find((item) => item.id !== task.id)?.id ?? null;
      setTasks((current) => current.filter((item) => item.id !== task.id));
      setTaskManagement((current) => current.filter((row) => row.task.id !== task.id));
      if (selectedId === task.id) {
        setSelectedProjectId(null);
        setSelectedId(nextTaskId);
      }
      await Promise.all([refreshQueueBase(), refreshTaskManagement()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setTaskMutationBusy(false);
    }
  }

  async function createSelectedTaskRework() {
    if (!selectedTask || selectedTask.state !== "done") return;
    const reason = window.prompt(t("reworkReasonPrompt"));
    if (!reason?.trim()) return;
    setTaskMutationBusy(true);
    try {
      const created = await api<{ task: Task }>(`/api/tasks/${selectedTask.id}/rework`, {
        method: "POST",
        body: JSON.stringify({
          reason: reason.trim(),
          title: locale === "zh-CN" ? `返工：${selectedTask.title}` : `Rework: ${selectedTask.title}`,
        }),
      });
      await refreshQueueBase();
      selectTask(created.task.id);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setTaskMutationBusy(false);
    }
  }

  async function reopenSelectedTask() {
    if (!selectedTask || selectedTask.state !== "done") return;
    const reason = window.prompt(t("reopenReasonPrompt"));
    if (!reason?.trim()) return;
    setTaskMutationBusy(true);
    try {
      const next = await api<Task>(`/api/tasks/${selectedTask.id}/reopen`, {
        method: "POST",
        body: JSON.stringify({ reason: reason.trim() }),
      });
      setTasks((current) => current.map((task) => task.id === next.id ? next : task));
      await Promise.all([refreshQueueBase(), refreshDetail()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setTaskMutationBusy(false);
    }
  }

  async function reloadCollectionDetail(chain: boolean, id: string) {
    const detail = await api<TaskCollectionDetail>(`/api/${chain ? "task-chains" : "task-groups"}/${id}`);
    if (chain) setChainDetail(detail);
    else setGroupDetail(detail);
    return detail;
  }

  async function createCollection(chain: boolean) {
    const rawName = chain ? newChainName : newGroupName;
    const name = rawName.trim();
    if (!name) return;
    setCollectionBusy(true);
    try {
      const created = await api<TaskCollection>(`/api/${chain ? "task-chains" : "task-groups"}`, {
        method: "POST",
        body: JSON.stringify({ name, description: "", archived: false }),
      });
      if (chain) {
        setNewChainName("");
        setSelectedChainId(created.id);
      } else {
        setNewGroupName("");
        setSelectedGroupId(created.id);
      }
      await refreshTaskManagement();
      await reloadCollectionDetail(chain, created.id);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCollectionBusy(false);
    }
  }

  async function archiveCollection(chain: boolean, id: string) {
    setCollectionBusy(true);
    try {
      await api<{ archived: boolean }>(`/api/${chain ? "task-chains" : "task-groups"}/${id}/archive`, { method: "POST" });
      await refreshTaskManagement();
      await reloadCollectionDetail(chain, id);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCollectionBusy(false);
    }
  }

  async function changeCollectionMember(chain: boolean, id: string, taskId: string, remove: boolean) {
    if (!taskId) return;
    setCollectionBusy(true);
    try {
      await api<{ ok: boolean }>(`/api/${chain ? "task-chains" : "task-groups"}/${id}/members/${taskId}`, {
        method: remove ? "DELETE" : "PUT",
        ...(chain && !remove ? { body: JSON.stringify({ join_mode: chainJoinMode }) } : {}),
      });
      if (chain) setChainMemberTaskId("");
      else setGroupMemberTaskId("");
      await reloadCollectionDetail(chain, id);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCollectionBusy(false);
    }
  }

  async function saveChainEdge() {
    if (!selectedChainId || !chainEdgePredecessorId || !chainEdgeSuccessorId) return;
    let condition: EdgeCondition = { kind: "unconditional" };
    if (chainEdgeConditionKind === "result_equals") {
      try {
        condition = { kind: "result_equals", path: chainEdgePath.trim(), value: JSON.parse(chainEdgeValue) };
      } catch {
        setError(locale === "zh-CN" ? `条件值必须是有效 JSON，例如 "success"、1、true 或 {"kind":"x"}。` : `Condition value must be valid JSON, e.g. "success", 1, true, or {"kind":"x"}.`);
        return;
      }
    }
    setCollectionBusy(true);
    try {
      await api<{ ok: boolean }>(`/api/task-chains/${selectedChainId}/edges`, {
        method: "POST",
        body: JSON.stringify({
          task_id: chainEdgeSuccessorId,
          depends_on_task_id: chainEdgePredecessorId,
          condition,
        }),
      });
      await reloadCollectionDetail(true, selectedChainId);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCollectionBusy(false);
    }
  }

  async function removeChainEdge(taskId: string, predecessorId: string) {
    if (!selectedChainId) return;
    setCollectionBusy(true);
    try {
      await api<{ ok: boolean }>(`/api/task-chains/${selectedChainId}/edges/${taskId}/${predecessorId}`, { method: "DELETE" });
      await reloadCollectionDetail(true, selectedChainId);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCollectionBusy(false);
    }
  }

  async function assignSelectedGroup() {
    if (!selectedGroupId || !groupAgentId) return;
    setCollectionBusy(true);
    try {
      const results = await api<BatchAssignmentResult[]>("/api/task-groups/batch-assign", {
        method: "POST",
        body: JSON.stringify({ group_id: selectedGroupId, agent_instance_id: groupAgentId, lease_seconds: 3600 }),
      });
      setGroupAssignResults(results);
      await Promise.all([reloadCollectionDetail(false, selectedGroupId), refreshTaskManagement()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setCollectionBusy(false);
    }
  }

  function actorDisplayName(actorId: string) {
    if (actorId === "human:webui") return locale === "zh-CN" ? "WebUI 用户" : "WebUI user";
    if (actorId === "human:local") return locale === "zh-CN" ? "本地用户" : "Local user";
    if (actorId.startsWith("agent:")) {
      const agentId = actorId.slice("agent:".length);
      const entry = agents.find((item) => item.instance.id === agentId);
      return entry ? cleanAgentDisplayName(entry, locale) : shortId(agentId);
    }
    return actorId;
  }

  function openTaskFromManagement(taskId: string) {
    selectTask(taskId);
    setView("queue");
  }

  function toggleTaskManagementSort(key: TaskManagementSortKey) {
    setTaskManagementSort((current) => current.key === key
      ? { key, direction: current.direction === "asc" ? "desc" : "asc" }
      : { key, direction: "asc" });
  }

  function taskManagementSortHeader(key: TaskManagementSortKey, label: string) {
    const active = taskManagementSort.key === key;
    const direction = active ? taskManagementSort.direction : null;
    return (
      <th aria-sort={active ? (direction === "asc" ? "ascending" : "descending") : "none"}>
        <button
          type="button"
          className={`task-sort-button${active ? " active" : ""}`}
          onClick={() => toggleTaskManagementSort(key)}
          title={locale === "zh-CN" ? `按${label}排序` : `Sort by ${label}`}
        >
          <span>{label}</span>
          <span className="task-sort-indicator" aria-hidden="true">{active ? (direction === "asc" ? "↑" : "↓") : "↕"}</span>
        </button>
      </th>
    );
  }

  function beginProjectsResize(event: ReactPointerEvent<HTMLDivElement>) {
    const grid = event.currentTarget.parentElement;
    if (!grid) return;
    event.preventDefault();

    const startX = event.clientX;
    const startWidth = projectsPaneWidth;
    const gridWidth = grid.getBoundingClientRect().width;
    const minProjectsWidth = 270;
    const minDetailWidth = 480;
    const maxProjectsWidth = Math.max(minProjectsWidth, Math.min(720, gridWidth - minDetailWidth - 8));

    const previousCursor = document.body.style.cursor;
    const previousUserSelect = document.body.style.userSelect;
    document.body.style.cursor = "col-resize";
    document.body.style.userSelect = "none";

    const onMove = (moveEvent: PointerEvent) => {
      const nextWidth = Math.min(maxProjectsWidth, Math.max(minProjectsWidth, startWidth + moveEvent.clientX - startX));
      setProjectsPaneWidth(Math.round(nextWidth));
    };
    const onUp = () => {
      document.body.style.cursor = previousCursor;
      document.body.style.userSelect = previousUserSelect;
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
    };

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp, { once: true });
  }

  const viewMeta = {
    sessions: {
      zh: "会话",
      en: "Sessions",
      descZh: "人类与特定 Agent 实例的一对一持久化沟通 · 按需加载历史 · 可靠投递",
      descEn: "Persistent Agent sessions · lazy history · durable delivery",
    },
    queue: {
      zh: "项目",
      en: "Projects",
      descZh: "按项目组织任务 · 上下文快照 · 调度 · 执行与恢复证据",
      descEn: "Project-organized tasks · context snapshots · dispatch · execution evidence",
    },
    tasks: {
      zh: "任务",
      en: "Tasks",
      descZh: "全局任务状态 · 接单与执行进度 · 发布人与负责人 · 时间与项目元数据",
      descEn: "Global task status · assignment and execution progress · ownership · timing and project metadata",
    },
    agents: {
      zh: "Agent 集群",
      en: "Agent Fleet",
      descZh: "Agent 团队状态 · 工作能力 · 身份与运行位置",
      descEn: "Team status · work capacity · identity and runtime location",
    },
  }[view];

  return (
    <div className={`app-shell${sidebarCollapsed ? " sidebar-collapsed" : ""}`}>
      <aside className={`sidebar${sidebarCollapsed ? " is-collapsed" : ""}`}>
        <div className="brand">
          <MorrowsMark />
          <div className="brand-copy">
            <strong>Morrows</strong>
            <span>Agent Work OS</span>
          </div>
          <button
            type="button"
            className="sidebar-toggle"
            onClick={() => setSidebarCollapsed((value) => !value)}
            aria-label={sidebarCollapsed
              ? (locale === "zh-CN" ? "展开侧栏" : "Expand sidebar")
              : (locale === "zh-CN" ? "收起侧栏" : "Collapse sidebar")}
            title={sidebarCollapsed
              ? (locale === "zh-CN" ? "展开侧栏" : "Expand sidebar")
              : (locale === "zh-CN" ? "收起侧栏" : "Collapse sidebar")}
          >
            <SidebarIcon name={sidebarCollapsed ? "expand" : "collapse"} />
          </button>
        </div>

        <nav className="side-nav">
          <div className="nav-group">{locale === "zh-CN" ? "工作 WORK" : "WORK"}</div>
          <button
            className={view === "queue" ? "nav-active" : ""}
            onClick={() => setView("queue")}
            aria-current={view === "queue" ? "page" : undefined}
            title={sidebarCollapsed ? t("projects") : undefined}
          >
            <span className="nav-label"><span className="nav-icon"><SidebarIcon name="project" /></span><span className="nav-text">{t("projects")}</span></span>
            <span className="nav-count">{projects.length}</span>
          </button>
          <button
            className={view === "tasks" ? "nav-active" : ""}
            onClick={() => setView("tasks")}
            aria-current={view === "tasks" ? "page" : undefined}
            title={sidebarCollapsed ? t("tasks") : undefined}
          >
            <span className="nav-label"><span className="nav-icon"><SidebarIcon name="task" /></span><span className="nav-text">{t("tasks")}</span></span>
            <span className="nav-count">{tasks.length}</span>
          </button>
          <button
            className={view === "sessions" ? "nav-active" : ""}
            onClick={() => setView("sessions")}
            aria-current={view === "sessions" ? "page" : undefined}
            title={sidebarCollapsed ? t("sessions") : undefined}
          >
            <span className="nav-label"><span className="nav-icon"><SidebarIcon name="session" /></span><span className="nav-text">{t("sessions")}</span></span>
          </button>

          <div className="nav-group">{locale === "zh-CN" ? "资源 RESOURCES" : "RESOURCES"}</div>
          <button
            className={view === "agents" ? "nav-active" : ""}
            onClick={() => setView("agents")}
            aria-current={view === "agents" ? "page" : undefined}
            title={sidebarCollapsed ? t("agentFleet") : undefined}
          >
            <span className="nav-label"><span className="nav-icon"><SidebarIcon name="agent" /></span><span className="nav-text">{t("agentFleet")}</span></span>
            <span className="nav-count">{agents.filter((a) => a.instance.status === "online").length}</span>
          </button>
        </nav>

        <div className="sidebar-spacer" />
        <div className={`system-card ${systemHealth?.ok ? "system-online" : healthCheckedAt ? "system-offline" : "system-checking"}`}>
          <span className="status-dot" />
          <div>
            <strong>{t("localDaemon")}</strong>
            <small>
              {systemHealth?.ok
                ? `127.0.0.1:8787 · ${t("online")}`
                : healthCheckedAt
                  ? t("unreachable")
                  : t("checking")}
            </small>
          </div>
        </div>
        <div className="planes-tip">{locale === "zh-CN" ? "控制面 · LSM 运行时 · Provider" : "Control · LSM runtime · Provider"}</div>
      </aside>

      <div className="app-main">
        <header className="topbar">
          <div className="topbar-title">
            <div className="crumb">
              <strong>{viewMeta.zh}</strong>
              <span>{viewMeta.en}</span>
            </div>
            <p>{locale === "zh-CN" ? viewMeta.descZh : viewMeta.descEn}</p>
          </div>
          <div className="topbar-actions">
            <div
              className={`sync-pill ${systemHealth?.ok ? "sync-online" : "sync-offline"}`}
              title={healthCheckedAt ? `${t("lastChecked")} ${healthCheckedAt.toLocaleTimeString(locale)}` : t("checking")}
            >
              <span className="sync-dot" />
              {systemHealth?.ok ? (locale === "zh-CN" ? "数据同步正常" : "Synced") : t("unreachable")}
            </div>
            <button
              type="button"
              className="icon-button"
              title={locale === "zh-CN" ? "刷新数据" : "Refresh"}
              onClick={() => {
                void refreshHealth();
                void refreshFleet();
                if (view === "queue") {
                  void refreshQueueBase();
                  void refreshDetail();
                }
                if (view === "tasks") void refreshTaskManagement();
              }}
            >
              ↻
            </button>
            <button
              type="button"
              className="icon-button"
              title={locale === "zh-CN" ? "切换深浅主题" : "Toggle theme"}
              onClick={() => setTheme((current) => current === "dark" ? "light" : "dark")}
            >
              {theme === "dark" ? "☼" : "◐"}
            </button>
            <div className="settings-wrap">
              <button
                type="button"
                className={`icon-button ${showSettings ? "settings-active" : ""}`}
                title={t("settings")}
                aria-expanded={showSettings}
                onClick={() => setShowSettings((current) => !current)}
              >
                ⚙
              </button>
              {showSettings && (
                <div className="settings-menu">
                  <div className="settings-menu-head">
                    <div>
                      <strong>{t("settings")}</strong>
                      <small>{t("settingsHint")}</small>
                    </div>
                    <button
                      type="button"
                      className="settings-close"
                      title={t("close")}
                      onClick={() => setShowSettings(false)}
                    >
                      ×
                    </button>
                  </div>
                  <label className="font-scale-setting">
                    <div>
                      <span>{t("fontScale")}</span>
                      <strong>{Math.round(fontScale * 100)}%</strong>
                    </div>
                    <input
                      type="range"
                      min="85"
                      max="150"
                      step="5"
                      value={Math.round(fontScale * 100)}
                      onChange={(event) => setFontScale(Number(event.target.value) / 100)}
                    />
                    <div className="font-scale-marks">
                      <span>85%</span>
                      <span>115%</span>
                      <span>150%</span>
                    </div>
                  </label>
                  <button
                    type="button"
                    className="settings-reset"
                    onClick={() => setFontScale(1.15)}
                  >
                    {t("resetFontScale")}
                  </button>
                  <p className="settings-note">{t("fontScaleExclusion")}</p>
                </div>
              )}
            </div>
            <OperatorLogin locale={locale} onLogin={() => {
              void refreshFleet();
              void refreshQueueBase();
              void refreshTaskManagement();
              void refreshSessionScopes();
              if (selectedId) void refreshDetail();
            }} />
            <button
              type="button"
              className="language-toggle"
              title={t("language")}
              onClick={() => setLocale((current) => current === "zh-CN" ? "en" : "zh-CN")}
            >
              {locale === "zh-CN" ? "中文 · EN" : "EN · 中文"}
            </button>
            <div
              className={`mcp-pill ${systemHealth?.mcp?.ready ? "mcp-ready" : healthCheckedAt ? "mcp-unavailable" : "mcp-checking"}`}
            >
              MCP {systemHealth?.mcp?.ready ? t("ready") : healthCheckedAt ? t("unavailable") : t("checking")}
            </div>
          </div>
        </header>

        <main className="content">

        {error && <div className="error-banner">{error}</div>}

        {view === "tasks" ? (
          <section className="task-management-page">
            <div className="task-management-tabs" role="tablist">
              <button type="button" className={taskManagementMode === "tasks" ? "selected" : ""} onClick={() => setTaskManagementMode("tasks")}>
                {locale === "zh-CN" ? "任务" : "Tasks"} <span>{taskManagement.length}</span>
              </button>
              <button type="button" className={taskManagementMode === "chains" ? "selected" : ""} onClick={() => setTaskManagementMode("chains")}>
                {locale === "zh-CN" ? "任务链" : "Task Chains"} <span>{taskChains.length}</span>
              </button>
              <button type="button" className={taskManagementMode === "groups" ? "selected" : ""} onClick={() => setTaskManagementMode("groups")}>
                {locale === "zh-CN" ? "任务组" : "Task Groups"} <span>{taskGroups.length}</span>
              </button>
            </div>

            {taskManagementMode === "tasks" && (<>
            <div className="task-management-summary">
              {(["all", "unclaimed", "assigned", "running", "review", "blocked", "done", "cancelled"] as TaskManagementFilter[]).map((status) => (
                <button
                  type="button"
                  key={status}
                  className={`task-summary-filter task-status-${status}${taskManagementFilter === status ? " selected" : ""}`}
                  onClick={() => setTaskManagementFilter(status)}
                >
                  <span>{taskManagementStatusLabel(locale, status)}</span>
                  <strong>{taskManagementCounts[status]}</strong>
                </button>
              ))}
            </div>

            <div className="task-management-toolbar">
              <div>
                <strong>{locale === "zh-CN" ? "全部任务" : "All tasks"}</strong>
                <small>
                  {locale === "zh-CN"
                    ? `显示 ${filteredTaskManagement.length} / ${taskManagement.length} 条`
                    : `Showing ${filteredTaskManagement.length} of ${taskManagement.length}`}
                </small>
              </div>
              <input
                type="search"
                value={taskManagementQuery}
                onChange={(event) => setTaskManagementQuery(event.target.value)}
                placeholder={locale === "zh-CN" ? "搜索任务、项目、发布人或接单人" : "Search task, project, publisher, or assignee"}
                aria-label={locale === "zh-CN" ? "搜索任务" : "Search tasks"}
              />
            </div>

            <div className="task-management-table-wrap">
              <table className="task-management-table">
                <thead>
                  <tr>
                    {taskManagementSortHeader("status", locale === "zh-CN" ? "状态" : "Status")}
                    {taskManagementSortHeader("task", locale === "zh-CN" ? "任务" : "Task")}
                    {taskManagementSortHeader("project", locale === "zh-CN" ? "项目" : "Project")}
                    {taskManagementSortHeader("publisher", locale === "zh-CN" ? "发布人" : "Publisher")}
                    {taskManagementSortHeader("assignee", locale === "zh-CN" ? "接单人" : "Assignee")}
                    {taskManagementSortHeader("created", locale === "zh-CN" ? "创建时间" : "Created")}
                    {taskManagementSortHeader("updated", locale === "zh-CN" ? "更新时间" : "Updated")}
                    {taskManagementSortHeader("priority", locale === "zh-CN" ? "优先级" : "Priority")}
                    <th>{locale === "zh-CN" ? "操作" : "Actions"}</th>
                  </tr>
                </thead>
                <tbody>
                  {filteredTaskManagement.map((row) => {
                    const status = taskManagementStatus(row);
                    const assignmentMode = row.task.assignment_mode === "open"
                      ? t("assignmentModeOpen")
                      : row.task.assignment_mode === "approval"
                        ? t("assignmentModeApproval")
                        : t("assignmentModeDispatch");
                    return (
                      <tr key={row.task.id}>
                        <td>
                          <span className={`task-management-state task-status-${status}`}>
                            <span className="task-management-state-dot" />
                            {taskManagementStatusLabel(locale, status)}
                          </span>
                          <small className="task-management-substate">
                            {row.latest_run_status
                              ? `Run · ${formatState(locale, row.latest_run_status)}`
                              : formatState(locale, row.task.state)}
                          </small>
                        </td>
                        <td className="task-management-title-cell">
                          <button type="button" onClick={() => openTaskFromManagement(row.task.id)}>
                            <strong>{row.task.title}</strong>
                            <span>{row.task.description || (locale === "zh-CN" ? "无描述" : "No description")}</span>
                          </button>
                          <small>{shortId(row.task.id)} · {assignmentMode}</small>
                        </td>
                        <td>
                          <strong>{row.project_name || (locale === "zh-CN" ? "未分类" : "Unclassified")}</strong>
                        </td>
                        <td>
                          <strong>{actorDisplayName(row.task.owner_actor_id)}</strong>
                          <small>{row.task.owner_actor_id}</small>
                        </td>
                        <td>
                          {row.executor_agent_display_name ? (
                            <>
                              <strong>{row.executor_agent_display_name}</strong>
                              <small>
                                {row.executor_assignment_status ? formatState(locale, row.executor_assignment_status) : ""}
                                {row.executor_acquired_at ? ` · ${formatDateTime(locale, row.executor_acquired_at)}` : ""}
                              </small>
                            </>
                          ) : (
                            <span className="task-unassigned-label">{locale === "zh-CN" ? "尚未接单" : "Unassigned"}</span>
                          )}
                        </td>
                        <td><time>{formatDateTime(locale, row.task.created_at)}</time></td>
                        <td><time>{formatDateTime(locale, row.task.updated_at)}</time></td>
                        <td><span className="task-priority-pill">P{row.task.priority}</span></td>
                        <td className="task-management-actions-cell">
                          {row.task.state !== "done" && row.task.state !== "cancelled" && (
                            <button className="secondary" type="button" onClick={() => void cancelTask(row.task)} disabled={taskMutationBusy}>
                              {t("cancelTask")}
                            </button>
                          )}
                          <button className="danger" type="button" onClick={() => void deleteTask(row.task)} disabled={taskMutationBusy}>
                            {t("deleteTask")}
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
              {!filteredTaskManagement.length && (
                <div className="empty large">
                  {locale === "zh-CN" ? "没有符合当前筛选条件的任务。" : "No tasks match the current filters."}
                </div>
              )}
            </div>
            </>)}

            {taskManagementMode === "chains" && (
              <div className="collection-management">
                <div className="collection-toolbar">
                  <select value={selectedChainId ?? ""} onChange={(event) => setSelectedChainId(event.target.value || null)}>
                    <option value="">{locale === "zh-CN" ? "选择任务链…" : "Select task chain…"}</option>
                    {taskChains.map((chain) => <option key={chain.id} value={chain.id}>{chain.name}{chain.archived ? " · archived" : ""}</option>)}
                  </select>
                  <input value={newChainName} onChange={(event) => setNewChainName(event.target.value)} placeholder={locale === "zh-CN" ? "新任务链名称" : "New chain name"} />
                  <button type="button" disabled={collectionBusy || !newChainName.trim()} onClick={() => void createCollection(true)}>{locale === "zh-CN" ? "创建" : "Create"}</button>
                  {chainDetail && !chainDetail.collection.archived && (
                    <button type="button" className="secondary" disabled={collectionBusy} onClick={() => void archiveCollection(true, chainDetail.collection.id)}>
                      {locale === "zh-CN" ? "归档" : "Archive"}
                    </button>
                  )}
                </div>

                {chainDetail ? (
                  <div className="collection-detail">
                    <div className="collection-heading">
                      <div>
                        <span className="section-caption">{locale === "zh-CN" ? "任务链 · DAG" : "Task Chain · DAG"}</span>
                        <h2>{chainDetail.collection.name}</h2>
                        <small>{chainDetail.members.length} {locale === "zh-CN" ? "个任务" : "tasks"} · {chainDetail.edges.length} {locale === "zh-CN" ? "条边" : "edges"}</small>
                      </div>
                    </div>

                    {!chainDetail.collection.archived && (
                      <div className="collection-form-row">
                        <select value={chainMemberTaskId} onChange={(event) => setChainMemberTaskId(event.target.value)}>
                          <option value="">{locale === "zh-CN" ? "添加成员任务…" : "Add member task…"}</option>
                          {tasks.filter((task) => !chainDetail.members.some((member) => member.id === task.id)).map((task) => <option key={task.id} value={task.id}>{task.title}</option>)}
                        </select>
                        <select value={chainJoinMode} onChange={(event) => setChainJoinMode(event.target.value as "all" | "any")}>
                          <option value="all">ALL</option>
                          <option value="any">ANY</option>
                        </select>
                        <button type="button" disabled={collectionBusy || !chainMemberTaskId} onClick={() => void changeCollectionMember(true, chainDetail.collection.id, chainMemberTaskId, false)}>
                          {locale === "zh-CN" ? "加入" : "Add"}
                        </button>
                      </div>
                    )}

                    <div className="chain-node-list">
                      {chainDetail.members.map((member) => {
                        const gate = chainDetail.gates.find((item) => item.task_id === member.id);
                        return (
                          <div className={"chain-node gate-" + (gate?.state || "eligible")} key={member.id}>
                            <div>
                              <button type="button" className="text-button" onClick={() => openTaskFromManagement(member.id)}><strong>{member.title}</strong></button>
                              <small>{shortId(member.id)} · {(gate?.join_mode || "all").toUpperCase()} · {formatState(locale, member.state)}</small>
                              {gate?.predecessors.length ? <p>{gate.predecessors.map((edge) => edge.reason).join(" · ")}</p> : <p>{locale === "zh-CN" ? "起始节点" : "Root node"}</p>}
                            </div>
                            <span className={"chain-gate-pill gate-" + (gate?.state || "eligible")}>{gate?.state || "eligible"}</span>
                            {!chainDetail.collection.archived && <button type="button" className="secondary compact-action" disabled={collectionBusy} onClick={() => void changeCollectionMember(true, chainDetail.collection.id, member.id, true)}>{locale === "zh-CN" ? "移出" : "Remove"}</button>}
                          </div>
                        );
                      })}
                      {!chainDetail.members.length && <div className="empty compact">{locale === "zh-CN" ? "任务链还没有成员。" : "This chain has no members."}</div>}
                    </div>

                    {!chainDetail.collection.archived && (
                      <div className="chain-edge-editor">
                        <select value={chainEdgePredecessorId} onChange={(event) => setChainEdgePredecessorId(event.target.value)}>
                          <option value="">{locale === "zh-CN" ? "前置任务…" : "Predecessor…"}</option>
                          {chainDetail.members.map((task) => <option key={task.id} value={task.id}>{task.title}</option>)}
                        </select>
                        <span>→</span>
                        <select value={chainEdgeSuccessorId} onChange={(event) => setChainEdgeSuccessorId(event.target.value)}>
                          <option value="">{locale === "zh-CN" ? "后续任务…" : "Successor…"}</option>
                          {chainDetail.members.map((task) => <option key={task.id} value={task.id}>{task.title}</option>)}
                        </select>
                        <select value={chainEdgeConditionKind} onChange={(event) => setChainEdgeConditionKind(event.target.value as "unconditional" | "result_equals")}>
                          <option value="unconditional">{locale === "zh-CN" ? "无条件" : "Unconditional"}</option>
                          <option value="result_equals">result ==</option>
                        </select>
                        {chainEdgeConditionKind === "result_equals" && (<>
                          <input value={chainEdgePath} onChange={(event) => setChainEdgePath(event.target.value)} placeholder="/kind" />
                          <input className="mono-input" value={chainEdgeValue} onChange={(event) => setChainEdgeValue(event.target.value)} placeholder={'"success"'} />
                        </>)}
                        <button type="button" disabled={collectionBusy || !chainEdgePredecessorId || !chainEdgeSuccessorId || chainEdgePredecessorId === chainEdgeSuccessorId} onClick={() => void saveChainEdge()}>
                          {locale === "zh-CN" ? "保存依赖" : "Save edge"}
                        </button>
                      </div>
                    )}

                    <div className="chain-edge-list">
                      {chainDetail.edges.map((edge) => (
                        <div className="chain-edge-row" key={edge.depends_on_task_id + ":" + edge.task_id}>
                          <button type="button" className="text-button" onClick={() => openTaskFromManagement(edge.depends_on_task_id)}>{tasks.find((task) => task.id === edge.depends_on_task_id)?.title || shortId(edge.depends_on_task_id)}</button>
                          <span>→</span>
                          <button type="button" className="text-button" onClick={() => openTaskFromManagement(edge.task_id)}>{tasks.find((task) => task.id === edge.task_id)?.title || shortId(edge.task_id)}</button>
                          <code>{edge.condition.kind === "unconditional" ? "always" : edge.condition.path + " == " + JSON.stringify(edge.condition.value)}</code>
                          {!chainDetail.collection.archived && <button type="button" className="secondary compact-action" disabled={collectionBusy} onClick={() => void removeChainEdge(edge.task_id, edge.depends_on_task_id)}>{locale === "zh-CN" ? "删除" : "Remove"}</button>}
                        </div>
                      ))}
                    </div>
                  </div>
                ) : <div className="empty large">{locale === "zh-CN" ? "选择或创建一个任务链。" : "Select or create a task chain."}</div>}
              </div>
            )}

            {taskManagementMode === "groups" && (
              <div className="collection-management">
                <div className="collection-toolbar">
                  <select value={selectedGroupId ?? ""} onChange={(event) => { setSelectedGroupId(event.target.value || null); setGroupAssignResults([]); }}>
                    <option value="">{locale === "zh-CN" ? "选择任务组…" : "Select task group…"}</option>
                    {taskGroups.map((group) => <option key={group.id} value={group.id}>{group.name}{group.archived ? " · archived" : ""}</option>)}
                  </select>
                  <input value={newGroupName} onChange={(event) => setNewGroupName(event.target.value)} placeholder={locale === "zh-CN" ? "新任务组名称" : "New group name"} />
                  <button type="button" disabled={collectionBusy || !newGroupName.trim()} onClick={() => void createCollection(false)}>{locale === "zh-CN" ? "创建" : "Create"}</button>
                  {groupDetail && !groupDetail.collection.archived && <button type="button" className="secondary" disabled={collectionBusy} onClick={() => void archiveCollection(false, groupDetail.collection.id)}>{locale === "zh-CN" ? "归档" : "Archive"}</button>}
                </div>

                {groupDetail ? (
                  <div className="collection-detail">
                    <div className="collection-heading">
                      <div>
                        <span className="section-caption">{locale === "zh-CN" ? "任务组 · 轻量集合" : "Task Group · Collection"}</span>
                        <h2>{groupDetail.collection.name}</h2>
                        <small>{locale === "zh-CN" ? "任务组不创建依赖；每个成员仍保持独立 Assignment / Run。" : "Groups create no dependencies; every member keeps an independent Assignment / Run."}</small>
                      </div>
                    </div>

                    {!groupDetail.collection.archived && (
                      <div className="collection-form-row">
                        <select value={groupMemberTaskId} onChange={(event) => setGroupMemberTaskId(event.target.value)}>
                          <option value="">{locale === "zh-CN" ? "添加任务…" : "Add task…"}</option>
                          {tasks.filter((task) => !groupDetail.members.some((member) => member.id === task.id)).map((task) => <option key={task.id} value={task.id}>{task.title}</option>)}
                        </select>
                        <button type="button" disabled={collectionBusy || !groupMemberTaskId} onClick={() => void changeCollectionMember(false, groupDetail.collection.id, groupMemberTaskId, false)}>{locale === "zh-CN" ? "加入" : "Add"}</button>
                      </div>
                    )}

                    <div className="group-member-list">
                      {groupDetail.members.map((member) => {
                        const gate = groupDetail.gates.find((item) => item.task_id === member.id);
                        return (
                          <div className="group-member-row" key={member.id}>
                            <div>
                              <button type="button" className="text-button" onClick={() => openTaskFromManagement(member.id)}><strong>{member.title}</strong></button>
                              <small>{shortId(member.id)} · {formatState(locale, member.state)}</small>
                            </div>
                            <span className={"chain-gate-pill gate-" + (gate?.state || "eligible")}>{gate?.state || "eligible"}</span>
                            {!groupDetail.collection.archived && <button type="button" className="secondary compact-action" disabled={collectionBusy} onClick={() => void changeCollectionMember(false, groupDetail.collection.id, member.id, true)}>{locale === "zh-CN" ? "移出" : "Remove"}</button>}
                          </div>
                        );
                      })}
                    </div>

                    {!groupDetail.collection.archived && (
                      <div className="group-assignment-panel">
                        <div>
                          <strong>{locale === "zh-CN" ? "批量指定 Agent" : "Assign group to Agent"}</strong>
                          <small>{locale === "zh-CN" ? "链 Gate 阻塞的任务会单独返回 blocked，不会绕过依赖。" : "Chain-gated tasks return blocked individually; dependencies are never bypassed."}</small>
                        </div>
                        <select value={groupAgentId} onChange={(event) => setGroupAgentId(event.target.value)}>
                          <option value="">{locale === "zh-CN" ? "选择 Agent…" : "Select Agent…"}</option>
                          {orderedAgents.filter((entry) => !entry.instance.archived_at).map((entry) => <option key={entry.instance.id} value={entry.instance.id}>{cleanAgentDisplayName(entry, locale)}</option>)}
                        </select>
                        <button type="button" disabled={collectionBusy || !groupAgentId || !groupDetail.members.length} onClick={() => void assignSelectedGroup()}>{locale === "zh-CN" ? "批量分配" : "Assign all"}</button>
                      </div>
                    )}

                    {!!groupAssignResults.length && (
                      <div className="group-assignment-results">
                        {groupAssignResults.map((result) => (
                          <div className={"group-assignment-result result-" + result.status} key={result.task_id}>
                            <strong>{tasks.find((task) => task.id === result.task_id)?.title || shortId(result.task_id)}</strong>
                            <span>{result.status}</span>
                            {result.error && <small>{result.error}</small>}
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                ) : <div className="empty large">{locale === "zh-CN" ? "选择或创建一个任务组。" : "Select or create a task group."}</div>}
              </div>
            )}
          </section>
        ) : view === "queue" ? (
          <div
            className="workspace-grid"
            style={{ gridTemplateColumns: `${projectsPaneWidth}px 8px minmax(0, 1fr)` }}
          >
            <section className="queue-panel workspace-pane">
              <AssignmentRequests locale={locale} tasks={tasks} agents={agents.map(entry => entry.instance)} onSelectTask={selectTask} onResolved={async () => { await refreshQueueBase(); await refreshDetail(); }} />
              <div className="task-browser-head">
                <div>
                  <strong>{t("projects")}</strong>
                  <small>{projects.length} {t("projects")} · {tasks.length} {t("tasks")}</small>
                </div>
                <div className="task-browser-actions">
                  <label className="project-sort-control">
                    <span>{t("sortBy")}</span>
                    <select value={projectSort} onChange={(event) => setProjectSort(event.target.value as typeof projectSort)}>
                      <option value="updated_desc">{t("sortUpdatedDesc")}</option>
                      <option value="created_desc">{t("sortCreatedDesc")}</option>
                      <option value="name_asc">{t("sortNameAsc")}</option>
                      <option value="task_count_desc">{t("sortTaskCountDesc")}</option>
                    </select>
                  </label>
                  <button type="button" className="secondary" onClick={() => setShowProjectCreate((current) => !current)}>
                    {showProjectCreate ? t("cancel") : t("newProject")}
                  </button>
                  <button type="button" className="secondary" onClick={() => void dispatchNextTask()} disabled={dispatchBusy}>
                    {t("dispatchNext")}
                  </button>
                </div>
              </div>

              {showProjectCreate && (
                <form className="new-project" onSubmit={createProject}>
                  <input
                    value={projectName}
                    onChange={(event) => setProjectName(event.target.value)}
                    placeholder={t("projectName")}
                    autoFocus
                  />
                  <button type="submit" disabled={projectBusy || !projectName.trim()}>{t("create")}</button>
                </form>
              )}

              <form className="new-task" onSubmit={createTask}>
                <select
                  value={newTaskProjectId}
                  onChange={(event) => setNewTaskProjectId(event.target.value)}
                  title={t("project")}
                >
                  <option value="">{t("unboundProject")}</option>
                  {projects.map((project) => (
                    <option key={project.id} value={project.id}>{project.name}</option>
                  ))}
                </select>
                <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder={t("newTask")} />
                <input
                  className="priority-input"
                  type="number"
                  value={priority}
                  onChange={(e) => setPriority(Number(e.target.value))}
                  title={t("priority")}
                />
                <button type="submit">{t("create")}</button>
              </form>

              <div className="task-list project-task-list">
                {projectGroups.map((group) => (
                  <section className="project-group" key={group.id}>
                    <button
                      type="button"
                      className={`project-group-head ${
                        selectedProjectId === group.id ? "selected" : ""
                      }`}
                      onClick={() => {
                        setExpandedProjectId((current) => current === group.id ? null : group.id);
                        if (group.project) void openProject(group.project);
                        else openUnclassified();
                      }}
                      aria-expanded={expandedProjectId === group.id}
                    >
                      <div className="project-folder">⌑</div>
                      <div>
                        <strong>{group.project?.name || t("unclassified")}</strong>
                        <small>
                          {group.project?.description || (group.project ? t("activeProject") : t("unclassifiedHint"))}
                        </small>
                      </div>
                      <div className="project-group-tail">
                        <span className="project-task-count">{group.tasks.length}</span>
                        <span className={`project-disclosure${expandedProjectId === group.id ? " expanded" : ""}`} aria-hidden="true">›</span>
                      </div>
                    </button>
                    {expandedProjectId === group.id && (
                      <div className="project-task-items">
                        {group.tasks.map((task) => (
                          <button
                            key={task.id}
                            className={`task-row ${!selectedProjectId && selectedId === task.id ? "selected" : ""}`}
                            onClick={() => selectTask(task.id)}
                          >
                            <div className="task-row-main">
                              <strong>{task.title}</strong>
                              <span className="task-id">{shortId(task.id)}</span>
                            </div>
                            <div className="task-meta">
                              <StateBadge state={task.state} locale={locale} />
                              {dispatchPolicies.some((policy) => policy.task_id === task.id && policy.role === "executor" && policy.enabled) && <span className="badge state-online">{t("dispatchEnabled")}</span>}
                              <span>P{task.priority}</span>
                            </div>
                          </button>
                        ))}
                        {!group.tasks.length && <div className="project-empty">{t("noProjectTasks")}</div>}
                      </div>
                    )}
                  </section>
                ))}
                {!projectGroups.length && <div className="empty">{t("noTasks")}</div>}
              </div>
            </section>

            <div
              className="workspace-resizer"
              role="separator"
              aria-orientation="vertical"
              aria-label={locale === "zh-CN" ? "调整项目栏与详情栏宽度" : "Resize Projects and detail panes"}
              tabIndex={0}
              onPointerDown={beginProjectsResize}
              onKeyDown={(event) => {
                if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
                event.preventDefault();
                const delta = event.key === "ArrowLeft" ? -16 : 16;
                setProjectsPaneWidth((width) => Math.min(720, Math.max(270, width + delta)));
              }}
            >
              <span />
            </div>

            <section className="detail-panel workspace-pane">
              {unclassifiedOpen ? (
                <>
                  <div className="detail-heading">
                    <div>
                      <span className="eyebrow">{t("project")}</span>
                      <h2>{t("unclassified")}</h2>
                    </div>
                    <span className="badge">{unclassifiedTasks.length} {t("tasks")}</span>
                  </div>
                  <p className="description">{t("unclassifiedHint")}</p>

                  <section className="task-metadata-card">
                    <div className="task-metadata-head">
                      <div>
                        <span className="section-caption">{t("unclassifiedGroup")}</span>
                        <strong>{t("unclassified")}</strong>
                      </div>
                    </div>
                    <div className="task-metadata-grid project-metadata-grid">
                      <div className="metadata-field">
                        <span>{t("tasks")}</span>
                        <strong>{unclassifiedTasks.length}</strong>
                      </div>
                      <div className="metadata-field metadata-id">
                        <span>{t("project")}</span>
                        <strong>{t("notAProject")}</strong>
                      </div>
                    </div>
                  </section>

                  <h3 className="detail-section-title tone-blue">{t("tasks")}</h3>
                  <div className="unclassified-task-grid">
                    {unclassifiedTasks.map((task) => (
                      <button
                        type="button"
                        className="unclassified-task-card"
                        key={task.id}
                        onClick={() => selectTask(task.id)}
                      >
                        <div className="mini-card-row">
                          <strong>{task.title}</strong>
                          <StateBadge state={task.state} locale={locale} />
                        </div>
                        <p>{task.description || t("noDescription")}</p>
                        <small>{shortId(task.id)} · P{task.priority} · {formatAge(locale, task.updated_at)}</small>
                      </button>
                    ))}
                    {!unclassifiedTasks.length && <div className="empty compact">{t("noUnclassifiedTasks")}</div>}
                  </div>

                  <h3 className="detail-section-title tone-violet">{t("projectMemories")}</h3>
                  <div className="empty compact">{t("unclassifiedNoMemory")}</div>
                </>
              ) : openedProject ? (
                <>
                  <div className="detail-heading">
                    <div>
                      <span className="eyebrow">{t("project")}</span>
                      <span className="task-id">{openedProject.id}</span>
                      <h2>{openedProject.name}</h2>
                    </div>
                    <StateBadge state={openedProject.status} locale={locale} />
                  </div>
                  <p className="description">{openedProject.description || t("noProjectDescription")}</p>

                  <section className="task-metadata-card">
                    <div className="task-metadata-head">
                      <div>
                        <span className="section-caption">{t("projectMetadata")}</span>
                        <strong>{openedProject.name}</strong>
                      </div>
                      <span className="task-id">{shortId(openedProject.id)}</span>
                    </div>
                    <div className="task-metadata-grid project-metadata-grid">
                      <div className="metadata-field">
                        <span>{t("state")}</span>
                        <StateBadge state={openedProject.status} locale={locale} />
                      </div>
                      <div className="metadata-field">
                        <span>{t("tasks")}</span>
                        <strong>{tasks.filter((task) => task.project_id === openedProject.id).length}</strong>
                      </div>
                      <div className="metadata-field">
                        <span>{t("memoryEntries")}</span>
                        <strong>{projectMemories.length}</strong>
                      </div>
                      <div className="metadata-field">
                        <span>{t("createdAt")}</span>
                        <strong>{formatDateTime(locale, openedProject.created_at)}</strong>
                      </div>
                      <div className="metadata-field">
                        <span>{t("updatedAt")}</span>
                        <strong>{formatDateTime(locale, openedProject.updated_at)}</strong>
                      </div>
                      <div className="metadata-field metadata-id">
                        <span>{t("projectId")}</span>
                        <code>{openedProject.id}</code>
                      </div>
                    </div>
                  </section>

                  <h3 className="detail-section-title tone-violet">{t("projectMemories")}</h3>
                  {projectMemoryLoading ? (
                    <div className="empty compact">{t("loadingMemories")}</div>
                  ) : projectMemories.length ? (
                    <div className="project-memory-layout">
                      <div className="project-memory-list">
                        {projectMemories.map((memory) => (
                          <button
                            type="button"
                            key={memory.id}
                            className={`project-memory-row ${selectedProjectMemory?.id === memory.id ? "selected" : ""}`}
                            onClick={() => setSelectedProjectMemoryId(memory.id)}
                          >
                            <strong>{memory.title}</strong>
                            <small>{memory.source_kind} · {formatAge(locale, memory.created_at)}</small>
                          </button>
                        ))}
                      </div>
                      {selectedProjectMemory && (
                        <article className="project-memory-detail">
                          <div className="project-memory-detail-head">
                            <div>
                              <span className="section-caption">{t("longTermMemory")}</span>
                              <h3>{selectedProjectMemory.title}</h3>
                            </div>
                            <span className="badge">{selectedProjectMemory.visibility}</span>
                          </div>
                          <dl className="memory-provenance">
                            <div><dt>{t("memorySource")}</dt><dd>{selectedProjectMemory.source_kind}</dd></div>
                            <div><dt>{t("sourceRef")}</dt><dd><code>{selectedProjectMemory.source_ref || "—"}</code></dd></div>
                            <div><dt>{t("createdAt")}</dt><dd>{new Date(selectedProjectMemory.created_at).toLocaleString(locale)}</dd></div>
                            {selectedProjectMemory.supersedes_memory_id && (
                              <div><dt>{t("supersedes")}</dt><dd><code>{selectedProjectMemory.supersedes_memory_id}</code></dd></div>
                            )}
                          </dl>
                          <pre className="memory-content">
                            {typeof selectedProjectMemory.content === "string"
                              ? selectedProjectMemory.content
                              : JSON.stringify(selectedProjectMemory.content, null, 2)}
                          </pre>
                          {selectedProjectMemory.provenance && <details>
                            <summary>{locale === "zh-CN" ? "来源与验证边界" : "Provenance and verification limits"}</summary>
                            <pre className="memory-content">{JSON.stringify(selectedProjectMemory.provenance, null, 2)}</pre>
                          </details>}
                        </article>
                      )}
                    </div>
                  ) : (
                    <div className="empty compact">{t("noProjectMemories")}</div>
                  )}
                </>
              ) : selectedTask ? (
                <>
                  <div className="detail-heading">
                    <div>
                      <span className="task-id">{selectedTask.id}</span>
                      <h2>{selectedTask.title}</h2>
                    </div>
                    <div className="detail-heading-actions">
                      <StateBadge state={selectedTask.state} locale={locale} />
                      {selectedTask.state !== "done" && selectedTask.state !== "cancelled" && (
                        <button className="secondary" type="button" onClick={() => void cancelTask(selectedTask)} disabled={taskMutationBusy}>
                          {t("cancelTask")}
                        </button>
                      )}
                      <button className="danger" type="button" onClick={() => void deleteTask(selectedTask)} disabled={taskMutationBusy}>
                        {t("deleteTask")}
                      </button>
                    </div>
                  </div>
                  <p className="description">{selectedTask.description || t("noDescription")}</p>

                  <section className="task-metadata-card">
                    <div className="task-metadata-head">
                      <div>
                        <span className="section-caption">{t("taskMetadata")}</span>
                        <strong>{selectedTask.title}</strong>
                      </div>
                      <span className="task-id">{shortId(selectedTask.id)}</span>
                    </div>
                    <div className="task-metadata-grid">
                      <label className="metadata-field metadata-project">
                        <span>{t("project")}</span>
                        <select
                          value={selectedTask.project_id || ""}
                          onChange={(event) => void moveTaskToProject(event.target.value)}
                          disabled={projectBusy}
                        >
                          <option value="">{t("unboundProject")}</option>
                          {projects.map((project) => (
                            <option key={project.id} value={project.id}>{project.name}</option>
                          ))}
                        </select>
                        {selectedProject?.description && <small>{selectedProject.description}</small>}
                      </label>
                      <div className="metadata-field">
                        <span>{t("state")}</span>
                        <StateBadge state={selectedTask.state} locale={locale} />
                      </div>
                      <label className="metadata-field">
                        <span>{t("assignmentMode")}</span>
                        <select
                          value={selectedTask.assignment_mode}
                          onChange={(event) => void changeTaskAssignmentMode(event.target.value as Task["assignment_mode"])}
                          disabled={assignmentModeBusy}
                        >
                          <option value="open">{t("assignmentModeOpen")}</option>
                          <option value="approval">{t("assignmentModeApproval")}</option>
                          <option value="dispatch">{t("assignmentModeDispatch")}</option>
                        </select>
                        <small>{t("assignmentModeHint")}</small>
                      </label>
                      <div className="metadata-field">
                        <span>{t("priority")}</span>
                        <strong>P{selectedTask.priority}</strong>
                      </div>
                      <div className="metadata-field">
                        <span>{t("owner")}</span>
                        <code>{selectedTask.owner_actor_id}</code>
                      </div>
                      <div className="metadata-field">
                        <span>{t("contextRevision")}</span>
                        <code>{selectedTask.current_context_revision_id ? shortId(selectedTask.current_context_revision_id) : "—"}</code>
                      </div>
                      <div className="metadata-field">
                        <span>{t("createdAt")}</span>
                        <strong>{formatDateTime(locale, selectedTask.created_at)}</strong>
                      </div>
                      <div className="metadata-field">
                        <span>{t("updatedAt")}</span>
                        <strong>{formatDateTime(locale, selectedTask.updated_at)}</strong>
                      </div>
                      <div className="metadata-field metadata-id">
                        <span>{t("taskId")}</span>
                        <code>{selectedTask.id}</code>
                      </div>
                    </div>
                    {selectedTask.state === "done" && (
                      <div className="task-completion-actions">
                        <div>
                          <strong>{t("completedTaskActions")}</strong>
                          <small>{t("completedTaskActionsHint")}</small>
                        </div>
                        <div className="task-completion-action-buttons">
                          <button onClick={() => void createSelectedTaskRework()} disabled={taskMutationBusy}>
                            {t("createRework")}
                          </button>
                          <button className="secondary" onClick={() => void reopenSelectedTask()} disabled={taskMutationBusy}>
                            {t("reopenTask")}
                          </button>
                        </div>
                      </div>
                    )}
                  </section>

                  {taskGate && (
                    <section className={"task-chain-gate-card gate-" + taskGate.state}>
                      <div className="task-chain-gate-head">
                        <div>
                          <span className="section-caption">{locale === "zh-CN" ? "任务链 Gate" : "Task Chain Gate"}</span>
                          <strong>{taskGate.state}</strong>
                          <small>{locale === "zh-CN" ? "汇合规则" : "Join"} · {taskGate.join_mode.toUpperCase()}</small>
                        </div>
                        <span className={"chain-gate-pill gate-" + taskGate.state}>{taskGate.state}</span>
                      </div>
                      {!!taskGate.predecessors.length && (
                        <div className="task-chain-gate-relations">
                          <strong>{locale === "zh-CN" ? "前置任务" : "Predecessors"}</strong>
                          {taskGate.predecessors.map((edge) => (
                            <div className="task-chain-gate-edge" key={edge.predecessor_id}>
                              <button type="button" className="text-button" onClick={() => selectTask(edge.predecessor_id)}>
                                {tasks.find((task) => task.id === edge.predecessor_id)?.title || shortId(edge.predecessor_id)}
                              </button>
                              <span className={"chain-gate-pill gate-" + edge.state}>{edge.state}</span>
                              <small>{edge.reason}</small>
                              <code>{edge.condition.kind === "unconditional" ? "always" : edge.condition.path + " == " + JSON.stringify(edge.condition.value)}</code>
                            </div>
                          ))}
                        </div>
                      )}
                      {!!taskGate.successors.length && (
                        <div className="task-chain-successors">
                          <strong>{locale === "zh-CN" ? "后续任务" : "Successors"}</strong>
                          {taskGate.successors.map((taskId) => (
                            <button type="button" className="text-button" key={taskId} onClick={() => selectTask(taskId)}>
                              {tasks.find((task) => task.id === taskId)?.title || shortId(taskId)}
                            </button>
                          ))}
                        </div>
                      )}
                    </section>
                  )}

                  <div className="detail-columns">
                    <div>
                      <h3 className="detail-section-title tone-blue">{t("context")}</h3>
                      {context ? (
                        <div className="context-card">
                          <div className="section-caption">{t("revision")} v{context.version}</div>
                          <strong>{context.goal || t("noExplicitGoal")}</strong>
                          <p>{context.current_summary || context.background || t("noSummary")}</p>
                          <small>{context.created_by_actor_id} · {formatAge(locale, context.created_at)}</small>
                        </div>
                      ) : (
                        <div className="empty compact">{t("noContext")}</div>
                      )}

                      <details className="context-editor">
                        <summary>{context ? t("reviseContext") : t("createContext")}</summary>
                        <form
                          className="context-editor-grid"
                          key={context?.id ?? selectedId ?? "new-context"}
                          onSubmit={(event) => void saveContext(event)}
                        >
                          <label>
                            <span>{t("contextGoal")}</span>
                            <input name="goal" defaultValue={context?.goal ?? ""} />
                          </label>
                          <label>
                            <span>{t("contextSummary")}</span>
                            <textarea name="current_summary" rows={3} defaultValue={context?.current_summary ?? ""} />
                          </label>
                          <label>
                            <span>{t("contextBackground")}</span>
                            <textarea name="background" rows={3} defaultValue={context?.background ?? ""} />
                          </label>
                          <label>
                            <span>{t("contextConstraints")}</span>
                            <textarea
                              name="constraints"
                              className="mono-input"
                              rows={4}
                              defaultValue={JSON.stringify(context?.constraints ?? {}, null, 2)}
                            />
                          </label>
                          <button type="submit" disabled={contextBusy}>{t("saveContext")}</button>
                        </form>
                      </details>

                      <h3 className="detail-section-title tone-violet">{t("contextPackage")}</h3>
                      <div className="context-package-card">
                        <div className="mini-card-row">
                          <strong>{contextPackage?.objective || t("noContextPackage")}</strong>
                          <button type="button" className="secondary" onClick={() => void assembleContextPackage()} disabled={contextBusy}>
                            {contextPackage ? t("refreshContextPackage") : t("assembleContextPackage")}
                          </button>
                        </div>
                        {contextPackage ? <>
                          <small>
                            {t("created")} {formatAge(locale, contextPackage.created_at)}
                            {contextPackage.context_snapshot_id ? ` · ${t("context")} ${shortId(contextPackage.context_snapshot_id)}` : ""}
                          </small>
                          {contextPackage.summary != null && <pre>{typeof contextPackage.summary === "string" ? contextPackage.summary : JSON.stringify(contextPackage.summary, null, 2)}</pre>}
                          {!!contextPackage.changed_files.length && <p><strong>{t("changedFiles")}：</strong>{contextPackage.changed_files.join(", ")}</p>}
                          {!!contextPackage.verified_results.length && <p><strong>{t("verifiedResults")}：</strong>{contextPackage.verified_results.join("; ")}</p>}
                          {!!contextPackage.blockers.length && <p><strong>{t("blockers")}：</strong>{contextPackage.blockers.join("; ")}</p>}
                          {!!contextPackage.unresolved_questions.length && <p><strong>{t("unresolvedQuestions")}：</strong>{contextPackage.unresolved_questions.join("; ")}</p>}
                          {contextPackage.next_action && <p><strong>{t("nextAction")}：</strong>{contextPackage.next_action}</p>}
                          <div className="context-package-refs">
                            <span>{t("memoryRefs")} {contextPackage.memory_refs.length}</span>
                            <span>{t("decisions")} {contextPackage.decision_refs.length}</span>
                            <span>{t("artifacts")} {contextPackage.artifact_refs.length}</span>
                          </div>
                        </> : <p>{t("contextPackageHint")}</p>}
                      </div>

                      <h3 className="detail-section-title tone-accent">{t("dispatcher")}</h3>
                      <div className="dispatch-card">
                        <div className="dispatch-form">
                          <label>
                            <span>{t("requiredCapabilities")}</span>
                            <input value={dispatchCapabilities} onChange={(e) => setDispatchCapabilities(e.target.value)} placeholder="code, rust, review" />
                          </label>
                          <label className="dispatch-small">
                            <span>{t("leaseSeconds")}</span>
                            <input type="number" min={30} value={dispatchLease} onChange={(e) => setDispatchLease(Number(e.target.value))} />
                          </label>
                          <label className="dispatch-check">
                            <input type="checkbox" checked={dispatchEnabled} onChange={(e) => setDispatchEnabled(e.target.checked)} />
                            {t("enabled")}
                          </label>
                        </div>
                        <div className="dispatch-actions">
                          <button onClick={() => void saveDispatchPolicy()} disabled={dispatchBusy}>{t("savePolicy")}</button>
                          <button className="secondary" onClick={() => void previewDispatch()} disabled={dispatchBusy || !dispatchPolicy}>{t("preview")}</button>
                          <button className="secondary" onClick={() => void dispatchSelectedTask()} disabled={dispatchBusy || !dispatchPolicy}>{t("dispatch")}</button>
                        </div>
                        {dispatchPolicy ? (
                          <small>{t("executor")} · {t("heartbeat")} ≤ {dispatchPolicy.heartbeat_ttl_seconds}s · {t("capacity")} ≤ {dispatchPolicy.capacity_ttl_seconds}s</small>
                        ) : <div className="empty compact">{t("noDispatchPolicy")}</div>}
                        {dispatchPreview && <div className="dispatch-preview">
                          <div className="mini-card-row">
                            <strong>{dispatchPreview.selected_agent_instance_id ? `${t("selected")} ${shortId(dispatchPreview.selected_agent_instance_id)}` : t("noEligibleAgent")}</strong>
                            <StateBadge state={dispatchPreview.task_dispatchable ? "ready" : "blocked"} locale={locale} />
                          </div>
                          {dispatchPreview.task_reasons.length > 0 && <p>{t("task")}：{dispatchPreview.task_reasons.map((reason) => formatDispatchReason(locale, reason)).join(locale === "zh-CN" ? "、" : ", ")}</p>}
                          {dispatchPreview.candidates.slice(0, 6).map((candidate) => (
                            <div className={`candidate-row ${candidate.eligible ? "candidate-ok" : ""}`} key={candidate.agent_instance_id}>
                              <div>
                                <strong>{candidate.agent_name}</strong>
                                <small>{shortId(candidate.agent_instance_id)} · {candidate.effective_slots} {t("effectiveSlots")} · {candidate.current_active_assignments} {t("active")}</small>
                              </div>
                              <span>{candidate.eligible ? t("eligible") : candidate.reasons.map((reason) => formatDispatchReason(locale, reason)).join(locale === "zh-CN" ? "、" : ", ")}</span>
                            </div>
                          ))}
                        </div>}
                        {dispatchDecisions.length > 0 && <div className="dispatch-history">
                          <strong>{t("decisionHistory")}</strong>
                          {dispatchDecisions.slice(0, 4).map((decision) => (
                            <small key={decision.id}>{formatOutcome(locale, decision.outcome)} · {decision.selected_agent_instance_id ? shortId(decision.selected_agent_instance_id) : t("noAgent")} · {formatAge(locale, decision.created_at)}</small>
                          ))}
                        </div>}
                      </div>

                      {assignmentIntake && (() => {
                        const interviewSession = taskSessions.find(
                          (session) => session.id === assignmentIntake.intake.interview_session_id,
                        );
                        const state = assignmentIntake.intake.conversation_state;
                        const statusText = state === "waiting_for_human"
                          ? (locale === "zh-CN" ? "Agent 正在等你回复" : "Agent is waiting for your reply")
                          : state === "waiting_for_agent"
                            ? (locale === "zh-CN" ? "你的回复已投递，Agent 会继续访谈" : "Your reply was delivered; the Agent will continue the interview")
                            : state === "converged"
                              ? (locale === "zh-CN" ? "访谈已收敛，正在切换到实施运行时" : "Interview converged; switching to the implementation runtime")
                              : assignmentIntake.assignment.phase === "context_review"
                                ? (locale === "zh-CN" ? "Agent 正在读取 Project Memory 与 Task Context" : "Agent is reviewing Project Memory and Task Context")
                                : (locale === "zh-CN" ? "准备开始 Human Interview" : "Preparing the Human Interview");
                        return (
                          <div className="human-interview-panel">
                            <div className="mini-card-row">
                              <div>
                                <h3 className="detail-section-title tone-violet">Human Interview</h3>
                                <small>{statusText}</small>
                              </div>
                              {interviewSession && (
                                <button
                                  type="button"
                                  className="secondary"
                                  onClick={() => {
                                    setSessionTargetId(interviewSession.id);
                                    setSessionTaskId(null);
                                    setSessionAgentId(null);
                                    setView("sessions");
                                  }}
                                >
                                  {locale === "zh-CN" ? "继续访谈" : "Continue interview"}
                                </button>
                              )}
                            </div>
                            <div className="intake-state-row">
                              <span>{locale === "zh-CN" ? "阶段" : "Phase"} · <code>{assignmentIntake.assignment.phase}</code></span>
                              <span>{locale === "zh-CN" ? "对话" : "Conversation"} · <code>{state}</code></span>
                            </div>
                            {interviewSession?.last_message_preview && (
                              <p className="human-interview-preview">{interviewSession.last_message_preview}</p>
                            )}
                            {!interviewSession && assignmentIntake.assignment.phase === "human_interview" && (
                              <small>{locale === "zh-CN" ? "访谈 Session 正在创建；无需手动审批。" : "The interview Session is being created; no manual approval is required."}</small>
                            )}
                          </div>
                        );
                      })()}

                      <div className="task-session-section">
                        <div className="mini-card-row">
                          <h3 className="detail-section-title tone-violet">{locale === "zh-CN" ? "相关会话" : "Related sessions"}</h3>
                          <button
                            type="button"
                            className="secondary"
                            onClick={() => {
                              setSessionTaskId(selectedTask.id);
                              setSessionTargetId(null);
                              setSessionAgentId(null);
                              setView("sessions");
                            }}
                          >
                            {locale === "zh-CN" ? "新建任务会话" : "New task session"}
                          </button>
                        </div>
                        <div className="stack">
                          {taskSessions.map((session) => {
                            const agent = orderedAgents.find((entry) => entry.instance.id === session.agent_instance_id);
                            return (
                              <button
                                type="button"
                                className="mini-card task-session-link"
                                key={session.id}
                                onClick={() => {
                                  setSessionTargetId(session.id);
                                  setSessionTaskId(null);
                                  setSessionAgentId(null);
                                  setView("sessions");
                                }}
                              >
                                <div className="mini-card-row">
                                  <strong>{session.title}</strong>
                                  <span>{session.queued_count > 0 ? (locale === "zh-CN" ? `${session.queued_count} 待回复` : `${session.queued_count} pending`) : ""}</span>
                                </div>
                                <small>{agent ? cleanAgentDisplayName(agent, locale) : session.agent_name} · {formatAge(locale, session.updated_at)}</small>
                                {session.last_message_preview && <p>{session.last_message_preview}</p>}
                              </button>
                            );
                          })}
                          {!taskSessions.length && (
                            <div className="empty compact">{locale === "zh-CN" ? "这个任务还没有会话。" : "No sessions are attached to this task yet."}</div>
                          )}
                        </div>
                      </div>

                      <h3 className="detail-section-title tone-green">{t("assignments")}</h3>
                      <div className="stack">
                        {assignments.map((item) => (
                          <div className="mini-card" key={item.id}>
                            <div><strong>{formatRole(locale, item.role)}</strong><StateBadge state={item.status} locale={locale} /></div>
                            <code>{shortId(item.agent_instance_id)}</code>
                            <small>{t("leaseUntil")} → {new Date(item.expires_at).toLocaleTimeString(locale)}</small>
                          </div>
                        ))}
                        {!assignments.length && <div className="empty compact">{t("unassigned")}</div>}
                      </div>

                      <h3 className="detail-section-title tone-accent">{t("launcher")}</h3>
                      <div className="stack">
                        {assignments
                          .filter((item) => item.role === "executor" && item.status === "active")
                          .map((item) => {
                            const profiles = launchProfiles.filter(
                              (profile) => profile.enabled && profile.agent_instance_id === item.agent_instance_id,
                            );
                            return (
                              <div className="context-card" key={"launch-" + item.id}>
                                <div className="mini-card-row">
                                  <strong>{shortId(item.agent_instance_id)}</strong>
                                  <small>{t("launchWith")}</small>
                                </div>
                                <div className="launch-actions">
                                  {profiles.map((profile) => (
                                    <div key={profile.id} className="launch-profile-actions">
                                      <button
                                        className="secondary"
                                        disabled={launchBusy}
                                        onClick={() => void enqueueLaunch(item.id, profile.id)}
                                      >
                                        {["codex_cli", "codebuddy_cli"].includes(profile.adapter) ? t("launch") : t("inviteExternal")} · {profile.name}
                                      </button>
                                      {["codex_cli", "codebuddy_cli"].includes(profile.adapter) && (() => {
                                        const previous = launchAttempts.find((attempt) =>
                                          attempt.launch_profile_id === profile.id &&
                                          attempt.agent_instance_id === item.agent_instance_id &&
                                          !!attempt.external_session_ref &&
                                          ["completed", "failed"].includes(attempt.status)
                                        );
                                        return previous && <button className="secondary" disabled={launchBusy} onClick={() => void enqueueLaunch(item.id, profile.id, previous.id)}>{t("resumeSession")}</button>;
                                      })()}
                                    </div>
                                  ))}
                                </div>
                                {!profiles.length && <div className="empty compact">{t("noLaunchProfile")}</div>}
                              </div>
                            );
                          })}
                        {!assignments.some((item) => item.role === "executor" && item.status === "active") && (
                          <div className="empty compact">{t("unassigned")}</div>
                        )}
                      </div>

                      <h3 className="detail-section-title tone-violet">{t("launchAttempts")}</h3>
                      <div className="stack">
                        {launchAttempts.map((attempt) => (
                          <div className="mini-card launch-attempt" key={attempt.id}>
                            <div className="mini-card-row">
                              <code>{shortId(attempt.id)}</code>
                              <StateBadge state={attempt.status} locale={locale} />
                            </div>
                            <small>{shortId(attempt.agent_instance_id)} · {attempt.cwd || "—"}</small>
                            {attempt.run_id && <small>Run {shortId(attempt.run_id)}</small>}
                            {attempt.external_session_ref && <small>{t("externalSession")} · {attempt.external_session_ref}</small>}
                            {attempt.pid !== null && attempt.pid !== undefined && <small>{t("processId")} · {attempt.pid}</small>}
                            {attempt.exit_code !== null && attempt.exit_code !== undefined && <small>{t("exitCode")} · {attempt.exit_code}</small>}
                            {attempt.stdout_path && <code>{t("stdoutLog")} · {attempt.stdout_path}</code>}
                            {attempt.stderr_path && <code>{t("stderrLog")} · {attempt.stderr_path}</code>}
                            {attempt.error && <p className="launch-error">{t("launchError")}：{attempt.error}</p>}
                            {attempt.resume_from_attempt_id && <small>{t("resumedFrom")} · {shortId(attempt.resume_from_attempt_id)}</small>}
                            {launchInstructions.filter((instruction) => instruction.launch_attempt_id === attempt.id).map((instruction) => (
                              <p key={instruction.id} className="launch-instruction">{instruction.body}</p>
                            ))}
                            {["queued", "starting", "running", "awaiting_agent"].includes(attempt.status) && <>
                              <div className="launch-instruction-form">
                                <input
                                  value={instructionDrafts[attempt.id] || ""}
                                  onChange={(event) => setInstructionDrafts((current) => ({ ...current, [attempt.id]: event.target.value }))}
                                  placeholder={t("instructionPlaceholder")}
                                  aria-label={t("instructionPlaceholder")}
                                />
                                <button className="secondary" disabled={launchBusy || !instructionDrafts[attempt.id]?.trim()} onClick={() => void sendLaunchInstruction(attempt.id)}>{t("sendInstruction")}</button>
                                <button className="secondary" disabled={launchBusy} onClick={() => void stopLaunch(attempt.id)}>{t("stopLaunch")}</button>
                              </div>
                            </>}
                          </div>
                        ))}
                        {!launchAttempts.length && <div className="empty compact">{t("noLaunchAttempts")}</div>}
                      </div>

                      <h3 className="detail-section-title tone-blue">{t("runs")}</h3>
                      <div className="stack">
                        {runs.map((run) => {
                          const execution = runExecutions[run.id];
                          const observation = execution?.observation;
                          return <div className="mini-card run-card" key={run.id}>
                            <div className="mini-card-row"><code>{shortId(run.id)}</code><StateBadge state={run.status} locale={locale} /></div>
                            <small>{run.external_session_ref || t("noExternalSession")} · {formatAge(locale, run.started_at)}</small>
                            {run.context_revision_id && <small>{t("pinnedContext")} · {shortId(run.context_revision_id)}</small>}
                            {(run.failure_reason || run.stop_reason) && <small>{formatState(locale, run.failure_reason || run.stop_reason || "")}</small>}
                            <div className="run-actions">
                              <button className="secondary" onClick={() => void toggleRunExecution(run.id)}>
                                {openRunId === run.id ? t("hideExecution") : t("inspectExecution")}
                              </button>
                              {run.status === "interrupted" && <button className="secondary" disabled={launchBusy}
                                onClick={() => void mutateRun(run.id, "restart")}>{t("restartRun")}</button>}
                              {["running", "interrupted"].includes(run.status) && <button className="secondary" disabled={launchBusy}
                                onClick={() => void mutateRun(run.id, "cancel")}>{t("cancelRun")}</button>}
                            </div>
                            {openRunId === run.id && <div className="run-execution">
                              {!execution && <small>{t("executionUnavailable")}</small>}
                              {execution?.binding && <>
                                <small>{t("lsmSession")} · <code>{execution.binding.logical_session_id}</code></small>
                                {execution.binding.restart_deadline_at && <small>{t("restartDeadline")} · {new Date(execution.binding.restart_deadline_at).toLocaleString(locale)}</small>}
                              </>}
                              {observation?.error && <p className="launch-error">{t("executionUnavailable")}：{observation.error}</p>}
                              {observation?.session && <small>{t("executionNodes")} · {observation.session.machines_touched.join(", ") || "local"}</small>}
                              {observation?.jobs && <section>
                                <h4>{t("executionJobs")}</h4>
                                {observation.jobs.jobs.length ? observation.jobs.jobs.map((job) => <div className="evidence-row" key={job.job_id}>
                                  <span><code>{job.job_id}</code> · {job.name} · {job.machine} · {formatState(locale, job.status)}</span>
                                  <button className="secondary" onClick={() => void loadJobOutput(run.id, job.job_id, job.machine)}>{t("showJobOutput")}</button>
                                  {jobOutputs[`${run.id}:${job.job_id}`] !== undefined && <pre>{jobOutputs[`${run.id}:${job.job_id}`]}</pre>}
                                </div>) : <small>{t("noExecutionEvidence")}</small>}
                                {!observation.jobs.complete && <small>{t("partialExecution")}：{observation.jobs.unreachable_machines.join(", ")}</small>}
                              </section>}
                              {observation?.shells && <section>
                                <h4>{t("executionShells")}</h4>
                                {observation.shells.shells.map((shell) => <div className="evidence-row" key={shell.session_id}>
                                  <code>{shell.session_id}</code> · {shell.machine} · {shell.backend || "shell"}
                                </div>)}
                                {!observation.shells.complete && <small>{t("partialExecution")}：{observation.shells.unreachable_machines.join(", ")}</small>}
                              </section>}
                              {execution?.evidence && <section>
                                <h4>{t("linkedEvidence")}</h4>
                                {execution.evidence.map((item) => <div className="evidence-row" key={item.id}>
                                  <code>{item.event_id ? shortId(item.event_id) : shortId(run.id)}</code> · {item.kind} · {item.reference}
                                  {item.start_seq !== null && item.start_seq !== undefined && ` · ${item.start_seq}–${item.end_seq ?? item.start_seq}`}
                                </div>)}
                              </section>}
                              {observation?.audit && <section>
                                <h4>{t("executionAudit")}</h4>
                                {observation.audit.entries.slice(0, 30).map((entry) => <details className="evidence-row" key={entry.id}>
                                  <summary>{entry.tool || entry.event} · {entry.status || ""} · {new Date(entry.ts * 1000).toLocaleString(locale)}</summary>
                                  <pre>{JSON.stringify({ input: entry.input, output: entry.output }, null, 2)}</pre>
                                </details>)}
                              </section>}
                            </div>}
                          </div>;
                        })}
                        {!runs.length && <div className="empty compact">{t("noRuns")}</div>}
                      </div>
                    </div>

                    <div>
                      {collaboration && <>
                        <h3 className="detail-section-title tone-violet">{t("handoffs")}</h3>
                        {collaboration.handoffs.map((handoff) => <div className="context-card" key={handoff.id}>
                          <div className="mini-card-row">
                            <strong>{handoff.summary}</strong>
                            <StateBadge state={handoff.status} locale={locale} />
                          </div>
                          <p>{t("completed")}：{handoff.completed.join("; ") || t("noneRecorded")}</p>
                          <p>{t("remaining")}：{handoff.remaining.join("; ")}</p>
                          <p>{t("blockers")}：{handoff.blockers.join("; ") || t("none")}</p>
                          <small>
                            {t("context")} {shortId(handoff.context_revision_id)}
                            {handoff.accepted_by_run_id ? ` · ${t("acceptedByRun")} ${shortId(handoff.accepted_by_run_id)}` : ""}
                          </small>
                        </div>)}
                        {!collaboration.handoffs.length && <div className="empty compact">{t("noHandoffs")}</div>}
                        <h3 className="detail-section-title tone-green">{t("artifacts")}</h3>
                        {collaboration.artifacts.map((artifact) => <div className="mini-card" key={artifact.id}>
                          <div className="mini-card-row"><strong>{artifact.title}</strong><span className="badge">{artifact.kind}</span></div>
                          <p>{artifact.description}</p><code>{artifact.uri}</code>
                        </div>)}
                        {!collaboration.artifacts.length && <div className="empty compact">{t("noArtifacts")}</div>}
                        <h3 className="detail-section-title tone-accent">{t("decisions")}</h3>
                        {collaboration.decisions.map((decision) => <div className="mini-card" key={decision.id}>
                          <strong>{decision.title}</strong><p>{decision.rationale}</p>
                        </div>)}
                        {!collaboration.decisions.length && <div className="empty compact">{t("noDecisions")}</div>}
                        <h3 className="detail-section-title tone-blue">{t("discussion")}</h3>
                        {collaboration.threads.map((thread) => <div className="context-card" key={thread.id}>
                          <strong>{thread.title}</strong>
                          {collaboration.messages.filter((message) => message.thread_id === thread.id).map((message) =>
                            <div className="message-row" key={message.id}>
                              <small>
                                {message.message_type} · {shortId(message.created_by)}
                                {message.recipient_agent_instance_id ? ` → ${shortId(message.recipient_agent_instance_id)}` : ""}
                                {message.recipient_role ? ` (${formatRole(locale, message.recipient_role)})` : ""}
                                {message.reply_to_message_id ? ` · ${t("replyTo")} ${shortId(message.reply_to_message_id)}` : ""}
                                {message.requires_response ? ` · ${t("responseRequired")}` : ""}
                              </small>
                              <p>{message.body}</p>
                            </div>)}
                        </div>)}
                        {!collaboration.threads.length && <div className="empty compact">{t("noDiscussions")}</div>}
                        <h3 className="detail-section-title tone-red">{t("prerequisites")}</h3>
                        {collaboration.dependencies.map((dependency) => <div key={dependency.depends_on_task_id}>
                          <button onClick={() => selectTask(dependency.depends_on_task_id)}>
                            {tasks.find((task) => task.id === dependency.depends_on_task_id)?.title || shortId(dependency.depends_on_task_id)}
                          </button>
                        </div>)}
                        {!collaboration.dependencies.length && <div className="empty compact">{t("noPrerequisites")}</div>}
                        <h3 className="detail-section-title tone-violet">{t("taskRelationships")}</h3>
                        {collaboration.relationships.map((relationship) => {
                          const outgoing = relationship.source_task_id === selectedTask.id;
                          const relatedTaskId = outgoing ? relationship.target_task_id : relationship.source_task_id;
                          const round = relationship.metadata.rework_round;
                          const reason = relationship.metadata.reason;
                          return <div className="context-card" key={`${relationship.source_task_id}:${relationship.target_task_id}:${relationship.relation_type}`}>
                            <div className="mini-card-row">
                              <strong>
                                {relationship.relation_type === "rework_of"
                                  ? (outgoing ? t("reworkOf") : t("reworkTask"))
                                  : relationship.relation_type}
                              </strong>
                              {typeof round === "number" && <span className="badge">#{round}</span>}
                            </div>
                            <button onClick={() => selectTask(relatedTaskId)}>
                              {tasks.find((task) => task.id === relatedTaskId)?.title || shortId(relatedTaskId)}
                            </button>
                            {typeof reason === "string" && <p>{reason}</p>}
                          </div>;
                        })}
                        {!collaboration.relationships.length && <div className="empty compact">{t("noTaskRelationships")}</div>}
                      </>}
                      <h3 className="detail-section-title tone-blue">{t("eventTimeline")}</h3>
                      <div className="timeline">
                        {events.map((event) => (
                          <div className="timeline-item" key={event.id}>
                            <span className="timeline-dot" />
                            <div>
                              <strong>{event.event_type}</strong>
                              <p>{event.actor_type}:{event.actor_id}</p>
                              <small>{new Date(event.created_at).toLocaleString(locale)}</small>
                            </div>
                          </div>
                        ))}
                        {!events.length && <div className="empty compact">{t("noEvents")}</div>}
                      </div>
                    </div>
                  </div>
                </>
              ) : (
                <div className="empty large">{t("selectTask")}</div>
              )}
            </section>
          </div>
        ) : view === "sessions" ? (
          <SessionChat
            agents={orderedAgents.map((entry) => ({
              id: entry.instance.id,
              name: cleanAgentDisplayName(entry, locale),
              status: entry.instance.status,
              account_id: entry.account?.id ?? null,
              account_email: entry.account?.email ?? null,
            }))}
            projects={projects.map((project) => ({ id: project.id, name: project.name }))}
            tasks={tasks.map((task) => ({ id: task.id, project_id: task.project_id, title: task.title }))}
            locale={locale}
            initialAgentId={sessionAgentId}
            initialSessionId={sessionTargetId}
            initialTaskId={sessionTaskId}
            onInitialAgentHandled={() => setSessionAgentId(null)}
            onInitialSessionHandled={() => setSessionTargetId(null)}
            onInitialTaskHandled={() => setSessionTaskId(null)}
          />
        ) : (
          <section className="fleet-page">
            <div className="fleet-overview">
              <div className="fleet-overview-copy">
                <span className="eyebrow">{t("agentFleet")}</span>
                <h2>{t("fleetOverview")}</h2>
                <p>{t("fleetOverviewHint")}</p>
                <button
                  type="button"
                  className="agent-add-button"
                  onClick={() => {
                    if (showAgentCreate) closeAgentCreate();
                    else setShowAgentCreate(true);
                  }}
                >
                  {t("addAgent")}
                </button>
              </div>
              <div className="fleet-stats">
                <div><span>{t("fleetTotal")}</span><strong>{fleetSummary.total}</strong></div>
                <div><span>{t("fleetOnline")}</span><strong>{fleetSummary.online}</strong></div>
                <div><span>{t("fleetWorking")}</span><strong>{fleetSummary.working}</strong></div>
                <div><span>{t("fleetAvailable")}</span><strong>{fleetSummary.available}</strong></div>
              </div>

              {showAgentCreate && (
                <div className="agent-create-panel">
                  {agentCreateResult ? (
                    <>
                      <div className="agent-create-head">
                        <div>
                          <span className="section-caption">{t("codexCredentialLinked")}</span>
                          <strong>{agentCreateResult.instance.display_name}</strong>
                          <p>{t("codexCredentialLinkedHint")}</p>
                        </div>
                        <button type="button" className="secondary" onClick={closeAgentCreate}>{t("closeAddAgent")}</button>
                      </div>
                      <dl className="agent-create-result-meta">
                        <div><dt>{t("accountEmail")}</dt><dd>{agentCreateResult.account.email || agentCreateResult.account.label}</dd></div>
                        <div><dt>{t("credentialKind")}</dt><dd><code>{agentCreateResult.credential_kind}</code></dd></div>
                        <div><dt>{t("credentialRef")}</dt><dd><code>{agentCreateResult.credential_ref}</code></dd></div>
                        <div><dt>{t("providerSecretStored")}</dt><dd>{agentCreateResult.credential_secret_stored ? t("yes") : t("no")}</dd></div>
                      </dl>
                    </>
                  ) : (
                    <form onSubmit={(event) => void createManagedCodexAgent(event)}>
                      <div className="agent-create-head">
                        <div>
                          <span className="section-caption">{t("addAgent")}</span>
                          <strong>Codex</strong>
                          <p>{t("addAgentHint")}</p>
                        </div>
                        <button type="button" className="secondary" onClick={closeAgentCreate}>{t("cancel")}</button>
                      </div>
                      <div className="agent-create-fields">
                        <label>
                          <span>{t("agentProvider")}</span>
                          <input value="Codex" disabled />
                        </label>
                        <label>
                          <span>{t("accountEmail")}</span>
                          <input
                            type="email"
                            value={newAgentEmail}
                            onChange={(event) => setNewAgentEmail(event.target.value)}
                            placeholder="name@example.com"
                            autoComplete="off"
                          />
                        </label>
                        <label>
                          <span>{t("credentialRef")}</span>
                          <input
                            value={newAgentCredentialRef}
                            onChange={(event) => setNewAgentCredentialRef(event.target.value)}
                            placeholder="~/.codex"
                            autoComplete="off"
                          />
                          <small>{t("credentialRefHint")}</small>
                        </label>
                        <label>
                          <span>{t("agentDisplayNameOptional")}</span>
                          <input
                            value={newAgentDisplayName}
                            onChange={(event) => setNewAgentDisplayName(event.target.value)}
                            maxLength={80}
                            placeholder="codex-1"
                          />
                          <small>{t("agentDisplayNameHint")}</small>
                        </label>
                        <label>
                          <span>{t("defaultModelOptional")}</span>
                          <input
                            value={newAgentModel}
                            onChange={(event) => setNewAgentModel(event.target.value)}
                            placeholder="gpt-5.6"
                          />
                        </label>
                      </div>
                      <div className="agent-create-actions">
                        <button type="submit" disabled={agentCreateBusy || !newAgentEmail.trim() || !newAgentCredentialRef.trim()}>
                          {agentCreateBusy ? t("creatingAgent") : t("createAgent")}
                        </button>
                      </div>
                    </form>
                  )}
                </div>
              )}
            </div>

            <div className="agent-grid">
              {orderedAgents.map((entry) => {
                const { instance: agent, profile, latest_capacity: capacity } = entry;
                const displayName = cleanAgentDisplayName(entry, locale);
                const legacy = profile.kind === "legacy" || profile.provider === "legacy";
                return (
                  <article className={`agent-card ${legacy ? "agent-card-legacy" : ""}`} key={agent.id}>
                    <div className="agent-card-head">
                      <div className="avatar">{displayName.slice(0, 2).toUpperCase()}</div>
                      <div className="agent-card-title">
                        {renamingAgentId === agent.id ? (
                          <form
                            className="agent-rename-form"
                            onSubmit={(event) => {
                              event.preventDefault();
                              void renameAgent(agent.id);
                            }}
                          >
                            <input
                              value={agentNameDraft}
                              onChange={(event) => setAgentNameDraft(event.target.value)}
                              maxLength={80}
                              autoFocus
                              aria-label={t("agentName")}
                            />
                            <button type="submit" disabled={agentRenameBusy || !agentNameDraft.trim()}>{t("save")}</button>
                            <button
                              type="button"
                              className="secondary"
                              onClick={() => {
                                setRenamingAgentId(null);
                                setAgentNameDraft("");
                              }}
                            >
                              {t("cancel")}
                            </button>
                          </form>
                        ) : (
                          <div className="agent-title-line">
                            <h2>{displayName}</h2>
                            <button
                              type="button"
                              className="agent-rename-button"
                              title={t("renameAgent")}
                              onClick={() => {
                                setRenamingAgentId(agent.id);
                                setAgentNameDraft(displayName);
                              }}
                            >
                              {locale === "zh-CN" ? "改名" : "Rename"}
                            </button>
                            {legacy && <span className="badge">{t("legacyRecord")}</span>}
                          </div>
                        )}
                        <span>{agentKindDisplayName(profile.kind, locale)} · {platformDisplayName(entry, locale)}</span>
                      </div>
                      <StateBadge state={agent.status} locale={locale} />
                    </div>

                    <div className="agent-work-state">
                      <div>
                        <span>{t("workState")}</span>
                        <strong>{capacity ? formatState(locale, capacity.status) : formatState(locale, agent.status)}</strong>
                      </div>
                      <div>
                        <span>{t("concurrency")}</span>
                        <strong>{capacity ? `${capacity.available_slots}${capacity.max_concurrency !== null ? ` / ${capacity.max_concurrency}` : ""}` : "—"}</strong>
                      </div>
                      <div>
                        <span>{t("currentTasks")}</span>
                        <strong>{capacity?.active_assignments ?? 0}</strong>
                      </div>
                      <div>
                        <span>{t("runningWork")}</span>
                        <strong>{capacity?.active_runs ?? 0}</strong>
                      </div>
                    </div>

                    <dl className="fleet-identity">
                      <div><dt>{t("agentType")}</dt><dd>{profile.name} · {agentKindDisplayName(profile.kind, locale)}</dd></div>
                      <div><dt>{t("agentName")}</dt><dd>{entry.reported_identity?.agent_name?.trim() || (locale === "zh-CN" ? "未上报" : "Not reported")}</dd></div>
                      <div><dt>{t("identity")}</dt><dd>{accountDisplayName(entry, locale)}</dd></div>
                      <div><dt>{t("runtimeLocation")}</dt><dd>{machineDisplayName(entry, locale)}</dd></div>
                    </dl>

                    <div className="agent-capability-section">
                      <span className="section-caption">{t("capabilities")}</span>
                      <div className="capability-list">
                        {agent.capabilities.length ? agent.capabilities.map((cap) => <span key={cap}>{cap.replaceAll("_", " ")}</span>) : <span>{t("general")}</span>}
                      </div>
                    </div>

                    <div className="agent-card-footer">
                      <div className="agent-freshness">
                        <span>{t("lastSeen")}</span>
                        <strong title={new Date(agent.last_heartbeat_at).toLocaleString(locale)}>{formatAge(locale, agent.last_heartbeat_at)}</strong>
                        {capacity?.quota_state && <small>{t("quota")} · {formatState(locale, capacity.quota_state)}</small>}
                      </div>
                      <button
                        type="button"
                        className="agent-chat-button"
                        onClick={() => {
                          setSessionAgentId(agent.id);
                          setSessionTargetId(null);
                          setSessionTaskId(null);
                          setView("sessions");
                        }}
                      >
                        {t("openSession")}
                      </button>
                    </div>

                    <details className="agent-technical">
                      <summary>{t("technicalInfo")}</summary>
                      <dl>
                        <div><dt>{t("instanceId")}</dt><dd><code>{agent.id}</code></dd></div>
                        <div><dt>{t("rawName")}</dt><dd><code>{agent.name}</code></dd></div>
                        <div><dt>{t("profile")}</dt><dd><code>{profile.name} · {shortId(profile.id)}</code></dd></div>
                        <div><dt>{t("account")}</dt><dd><code>{entry.account ? `${entry.account.label} · ${shortId(entry.account.id)}` : "—"}</code></dd></div>
                        <div><dt>{t("machine")}</dt><dd><code>{entry.machine ? `${entry.machine.name} · ${shortId(entry.machine.id)}` : "—"}</code></dd></div>
                        {agent.external_instance_ref && <div><dt>{t("externalRef")}</dt><dd><code>{agent.external_instance_ref}</code></dd></div>}
                      </dl>
                    </details>
                  </article>
                );
              })}
              {!agents.length && <div className="empty large">{t("noAgents")}</div>}
            </div>
          </section>
        )}
        </main>
      </div>
    </div>
  );
}
