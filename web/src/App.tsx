import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";
import "./App.css";
import {
  formatAge,
  formatDispatchReason,
  formatOutcome,
  formatRole,
  formatState,
  initialLocale,
  translate,
} from "./i18n";
import type { Locale, TranslationKey } from "./i18n";

type Task = {
  id: string;
  title: string;
  description: string;
  owner_actor_id: string;
  state: string;
  priority: number;
  current_context_revision_id?: string | null;
  created_at: string;
  updated_at: string;
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
};

type Agent = {
  id: string;
  name: string;
  status: string;
  capabilities: string[];
  last_heartbeat_at: string;
};

type FleetEntry = {
  instance: Agent;
  profile: { id: string; name: string; provider: string; kind: string };
  account: { id: string; label: string; provider: string; status: string } | null;
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
};

type Assignment = {
  id: string;
  task_id: string;
  role: string;
  agent_instance_id: string;
  status: string;
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
  checkpoint?: unknown;
  result?: unknown;
  started_at: string;
  ended_at?: string | null;
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
  codex_home?: string | null;
  default_cwd?: string | null;
  model?: string | null;
  enabled: boolean;
  metadata: unknown;
  created_at: string;
  updated_at: string;
};

type LaunchAttempt = {
  id: string;
  assignment_id: string;
  task_id: string;
  agent_instance_id: string;
  launch_profile_id: string;
  run_id?: string | null;
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

type LaunchInstruction = {
  id: string;
  launch_attempt_id: string;
  actor_type: string;
  actor_id: string;
  body: string;
  created_at: string;
};

async function api<T>(path: string, init?: RequestInit): Promise<T> {
  const response = await fetch(path, {
    ...init,
    headers: { "Content-Type": "application/json", ...(init?.headers || {}) },
  });
  if (!response.ok) {
    const text = await response.text();
    throw new Error(text || `${response.status} ${response.statusText}`);
  }
  return response.json();
}

function shortId(id: string) {
  return id.slice(0, 8);
}

function StateBadge({ state, locale }: { state: string; locale: Locale }) {
  return <span className={`badge state-${state}`}>{formatState(locale, state)}</span>;
}

export default function App() {
  const [locale, setLocale] = useState<Locale>(initialLocale);
  const [view, setView] = useState<"queue" | "agents">("queue");
  const [tasks, setTasks] = useState<Task[]>([]);
  const [agents, setAgents] = useState<FleetEntry[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [assignments, setAssignments] = useState<Assignment[]>([]);
  const [runs, setRuns] = useState<Run[]>([]);
  const [events, setEvents] = useState<EventItem[]>([]);
  const [collaboration, setCollaboration] = useState<Collaboration | null>(null);
  const [context, setContext] = useState<ContextRevision | null>(null);
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
  const [error, setError] = useState<string | null>(null);

  const selectedTask = useMemo(() => tasks.find((task) => task.id === selectedId) ?? null, [tasks, selectedId]);
  const t = (key: TranslationKey) => translate(locale, key);

  useEffect(() => {
    window.localStorage.setItem("morrows.locale", locale);
    document.documentElement.lang = locale;
  }, [locale]);

  const refreshBase = useCallback(async () => {
    try {
      const [nextTasks, nextAgents, nextPolicies, nextLaunchProfiles] = await Promise.all([
        api<Task[]>("/api/tasks"),
        api<FleetEntry[]>("/api/agent-fleet"),
        api<DispatchPolicy[]>("/api/dispatch-policies"),
        api<LaunchProfile[]>("/api/launch-profiles"),
      ]);
      setTasks(nextTasks);
      setAgents(nextAgents);
      setDispatchPolicies(nextPolicies);
      setLaunchProfiles(nextLaunchProfiles);
      if (!selectedId && nextTasks.length) setSelectedId(nextTasks[0].id);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [selectedId]);

  const refreshDetail = useCallback(async () => {
    if (!selectedId) return;
    try {
      const [nextAssignments, nextRuns, nextEvents, nextContext, nextCollaboration, nextPolicy, nextPreview, nextDispatchDecisions, nextLaunchAttempts, nextLaunchInstructions] = await Promise.all([
        api<Assignment[]>(`/api/tasks/${selectedId}/assignments`),
        api<Run[]>(`/api/tasks/${selectedId}/runs`),
        api<EventItem[]>(`/api/tasks/${selectedId}/events`),
        api<ContextRevision>(`/api/tasks/${selectedId}/context`).catch(() => null),
        api<Collaboration>(`/api/tasks/${selectedId}/collaboration`),
        api<DispatchPolicy>(`/api/tasks/${selectedId}/dispatch-policy/executor`).catch(() => null),
        api<DispatchPreview>(`/api/tasks/${selectedId}/dispatch-preview/executor`).catch(() => null),
        api<DispatchDecision[]>(`/api/tasks/${selectedId}/dispatch-decisions`),
        api<LaunchAttempt[]>(`/api/tasks/${selectedId}/launch-attempts`),
        api<LaunchInstruction[]>(`/api/tasks/${selectedId}/launch-instructions`),
      ]);
      setAssignments(nextAssignments);
      setRuns(nextRuns);
      setEvents(nextEvents);
      setContext(nextContext);
      setCollaboration(nextCollaboration);
      setDispatchPolicy(nextPolicy);
      setDispatchPreview(nextPreview);
      setDispatchDecisions(nextDispatchDecisions);
      setLaunchAttempts(nextLaunchAttempts);
      setLaunchInstructions(nextLaunchInstructions);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [selectedId]);

  useEffect(() => {
    void refreshBase();
    const timer = window.setInterval(() => void refreshBase(), 3000);
    return () => window.clearInterval(timer);
  }, [refreshBase]);

  useEffect(() => {
    void refreshDetail();
    const timer = window.setInterval(() => void refreshDetail(), 3000);
    return () => window.clearInterval(timer);
  }, [refreshDetail]);

  useEffect(() => {
    if (!selectedId) return;
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
  }, [selectedId]);

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
      await Promise.all([refreshBase(), refreshDetail()]);
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
      await Promise.all([refreshBase(), refreshDetail()]);
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
      await Promise.all([refreshBase(), refreshDetail()]);
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
      await Promise.all([refreshBase(), refreshDetail()]);
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
      await Promise.all([refreshBase(), refreshDetail()]);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLaunchBusy(false);
    }
  }

  async function createTask(event: FormEvent) {
    event.preventDefault();
    if (!title.trim()) return;
    try {
      const created = await api<Task>("/api/tasks", {
        method: "POST",
        body: JSON.stringify({ title: title.trim(), description: "", priority }),
      });
      setTitle("");
      setPriority(0);
      setSelectedId(created.id);
      await refreshBase();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand">
          <div className="brand-mark">M</div>
          <div>
            <strong>Morrows</strong>
            <span>{t("workOs")}</span>
          </div>
        </div>

        <nav>
          <button className={view === "queue" ? "nav-active" : ""} onClick={() => setView("queue")}>
            {t("workQueue")} <span>{tasks.length}</span>
          </button>
          <button className={view === "agents" ? "nav-active" : ""} onClick={() => setView("agents")}>
            {t("agentFleet")} <span>{agents.filter((a) => a.instance.status === "online").length}</span>
          </button>
        </nav>

        <div className="system-card">
          <span className="status-dot" />
          <div>
            <strong>{t("localDaemon")}</strong>
            <small>127.0.0.1:8787</small>
          </div>
        </div>
      </aside>

      <main>
        <header className="topbar">
          <div>
            <p className="eyebrow">{t("localFirstControlPlane")}</p>
            <h1>{view === "queue" ? t("workQueue") : t("agentFleet")}</h1>
          </div>
          <div className="topbar-actions">
            <button
              type="button"
              className="language-toggle"
              title={t("language")}
              onClick={() => setLocale((current) => current === "zh-CN" ? "en" : "zh-CN")}
            >
              {t("switchLanguage")}
            </button>
            <div className="mcp-pill">MCP /mcp</div>
          </div>
        </header>

        {error && <div className="error-banner">{error}</div>}

        {view === "queue" ? (
          <div className="workspace-grid">
            <section className="panel queue-panel">
              <form className="new-task" onSubmit={createTask}>
                <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder={t("newTask")} />
                <input
                  className="priority-input"
                  type="number"
                  value={priority}
                  onChange={(e) => setPriority(Number(e.target.value))}
                  title={t("priority")}
                />
                <button type="submit">{t("create")}</button>
                <button type="button" className="secondary" onClick={() => void dispatchNextTask()} disabled={dispatchBusy}>{t("dispatchNext")}</button>
              </form>

              <div className="task-list">
                {tasks.map((task) => (
                  <button
                    key={task.id}
                    className={`task-row ${selectedId === task.id ? "selected" : ""}`}
                    onClick={() => setSelectedId(task.id)}
                  >
                    <div className="task-row-main">
                      <span className="task-id">{shortId(task.id)}</span>
                      <strong>{task.title}</strong>
                    </div>
                    <div className="task-meta">
                      <StateBadge state={task.state} locale={locale} />
                      {dispatchPolicies.some((policy) => policy.task_id === task.id && policy.role === "executor" && policy.enabled) && <span className="badge state-online">{t("dispatchEnabled")}</span>}
                      <span>P{task.priority}</span>
                    </div>
                  </button>
                ))}
                {!tasks.length && <div className="empty">{t("noTasks")}</div>}
              </div>
            </section>

            <section className="panel detail-panel">
              {selectedTask ? (
                <>
                  <div className="detail-heading">
                    <div>
                      <span className="task-id">{selectedTask.id}</span>
                      <h2>{selectedTask.title}</h2>
                    </div>
                    <StateBadge state={selectedTask.state} locale={locale} />
                  </div>
                  <p className="description">{selectedTask.description || t("noDescription")}</p>

                  <div className="detail-columns">
                    <div>
                      <h3>{t("context")}</h3>
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

                      <h3>{t("dispatcher")}</h3>
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

                      <h3>{t("assignments")}</h3>
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

                      <h3>{t("launcher")}</h3>
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
                                        {profile.adapter === "codex_cli" ? t("launch") : t("inviteExternal")} · {profile.name}
                                      </button>
                                      {profile.adapter === "codex_cli" && (() => {
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

                      <h3>{t("launchAttempts")}</h3>
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

                      <h3>{t("runs")}</h3>
                      <div className="stack">
                        {runs.map((run) => (
                          <div className="mini-card" key={run.id}>
                            <div><code>{shortId(run.id)}</code><StateBadge state={run.status} locale={locale} /></div>
                            <small>{run.external_session_ref || t("noExternalSession")} · {formatAge(locale, run.started_at)}</small>
                          </div>
                        ))}
                        {!runs.length && <div className="empty compact">{t("noRuns")}</div>}
                      </div>
                    </div>

                    <div>
                      {collaboration && <>
                        <h3>{t("handoffs")}</h3>
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
                        <h3>{t("artifacts")}</h3>
                        {collaboration.artifacts.map((artifact) => <div className="mini-card" key={artifact.id}>
                          <div className="mini-card-row"><strong>{artifact.title}</strong><span className="badge">{artifact.kind}</span></div>
                          <p>{artifact.description}</p><code>{artifact.uri}</code>
                        </div>)}
                        {!collaboration.artifacts.length && <div className="empty compact">{t("noArtifacts")}</div>}
                        <h3>{t("decisions")}</h3>
                        {collaboration.decisions.map((decision) => <div className="mini-card" key={decision.id}>
                          <strong>{decision.title}</strong><p>{decision.rationale}</p>
                        </div>)}
                        {!collaboration.decisions.length && <div className="empty compact">{t("noDecisions")}</div>}
                        <h3>{t("discussion")}</h3>
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
                        <h3>{t("prerequisites")}</h3>
                        {collaboration.dependencies.map((dependency) => <div key={dependency.depends_on_task_id}>
                          <button onClick={() => setSelectedId(dependency.depends_on_task_id)}>
                            {tasks.find((task) => task.id === dependency.depends_on_task_id)?.title || shortId(dependency.depends_on_task_id)}
                          </button>
                        </div>)}
                        {!collaboration.dependencies.length && <div className="empty compact">{t("noPrerequisites")}</div>}
                      </>}
                      <h3>{t("eventTimeline")}</h3>
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
        ) : (
          <section className="agent-grid">
            {agents.map(({ instance: agent, profile, account, machine, latest_capacity: capacity }) => (
              <article className="agent-card" key={agent.id}>
                <div className="agent-card-head">
                  <div className="avatar">{agent.name.slice(0, 2).toUpperCase()}</div>
                  <div>
                    <h2>{agent.name}</h2>
                    <code>{shortId(agent.id)}</code>
                  </div>
                  <StateBadge state={agent.status} locale={locale} />
                </div>
                <dl className="fleet-identity">
                  <div><dt>{t("profile")}</dt><dd>{profile.name} · {profile.provider || t("unknownProvider")}{profile.kind ? ` · ${profile.kind}` : ""}</dd></div>
                  <div><dt>{t("account")}</dt><dd>{account ? `${account.label} · ${account.provider || t("unknownProvider")} · ${formatState(locale, account.status)}` : t("notLinked")}</dd></div>
                  <div><dt>{t("machine")}</dt><dd>{machine ? `${machine.name} · ${machine.hostname} · ${machine.os} ${machine.arch}` : t("notLinked")}</dd></div>
                </dl>
                <div className="capability-list">
                  {agent.capabilities.length ? agent.capabilities.map((cap) => <span key={cap}>{cap}</span>) : <span>{t("general")}</span>}
                </div>
                <small title={new Date(agent.last_heartbeat_at).toLocaleString(locale)}>{t("heartbeat")} {formatAge(locale, agent.last_heartbeat_at)}</small>
                <div className="fleet-capacity">
                  {capacity ? <>
                    <div className="mini-card-row"><strong>{t("capacity")}</strong><StateBadge state={capacity.status} locale={locale} /></div>
                    <p>{capacity.available_slots} {t("availableSlots")}{capacity.max_concurrency !== null ? ` / ${capacity.max_concurrency} ${t("max")}` : ` · ${t("maxNotReported")}`}</p>
                    <p>{capacity.active_assignments} {t("activeAssignments")} · {capacity.active_runs} {t("activeRuns")}</p>
                    <small>{t("quota")}：{capacity.quota_state ? formatState(locale, capacity.quota_state) : t("notReported")} · {t("observed")} {formatAge(locale, capacity.observed_at)}</small>
                  </> : <p>{t("noCapacity")}</p>}
                </div>
              </article>
            ))}
            {!agents.length && <div className="empty large">{t("noAgents")}</div>}
          </section>
        )}
      </main>
    </div>
  );
}
