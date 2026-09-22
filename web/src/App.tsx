import { useCallback, useEffect, useMemo, useState } from "react";
import type { FormEvent } from "react";
import "./App.css";

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

function age(iso: string) {
  const seconds = Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 1000));
  if (seconds < 60) return `${seconds}s ago`;
  if (seconds < 3600) return `${Math.floor(seconds / 60)}m ago`;
  return `${Math.floor(seconds / 3600)}h ago`;
}

function StateBadge({ state }: { state: string }) {
  return <span className={`badge state-${state}`}>{state.replaceAll("_", " ")}</span>;
}

export default function App() {
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
  const [dispatchCapabilities, setDispatchCapabilities] = useState("");
  const [dispatchEnabled, setDispatchEnabled] = useState(true);
  const [dispatchLease, setDispatchLease] = useState(900);
  const [dispatchBusy, setDispatchBusy] = useState(false);
  const [title, setTitle] = useState("");
  const [priority, setPriority] = useState(0);
  const [error, setError] = useState<string | null>(null);

  const selectedTask = useMemo(() => tasks.find((task) => task.id === selectedId) ?? null, [tasks, selectedId]);

  const refreshBase = useCallback(async () => {
    try {
      const [nextTasks, nextAgents, nextPolicies] = await Promise.all([
        api<Task[]>("/api/tasks"),
        api<FleetEntry[]>("/api/agent-fleet"),
        api<DispatchPolicy[]>("/api/dispatch-policies"),
      ]);
      setTasks(nextTasks);
      setAgents(nextAgents);
      setDispatchPolicies(nextPolicies);
      if (!selectedId && nextTasks.length) setSelectedId(nextTasks[0].id);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [selectedId]);

  const refreshDetail = useCallback(async () => {
    if (!selectedId) return;
    try {
      const [nextAssignments, nextRuns, nextEvents, nextContext, nextCollaboration, nextPolicy, nextPreview, nextDispatchDecisions] = await Promise.all([
        api<Assignment[]>(`/api/tasks/${selectedId}/assignments`),
        api<Run[]>(`/api/tasks/${selectedId}/runs`),
        api<EventItem[]>(`/api/tasks/${selectedId}/events`),
        api<ContextRevision>(`/api/tasks/${selectedId}/context`).catch(() => null),
        api<Collaboration>(`/api/tasks/${selectedId}/collaboration`),
        api<DispatchPolicy>(`/api/tasks/${selectedId}/dispatch-policy/executor`).catch(() => null),
        api<DispatchPreview>(`/api/tasks/${selectedId}/dispatch-preview/executor`).catch(() => null),
        api<DispatchDecision[]>(`/api/tasks/${selectedId}/dispatch-decisions`),
      ]);
      setAssignments(nextAssignments);
      setRuns(nextRuns);
      setEvents(nextEvents);
      setContext(nextContext);
      setCollaboration(nextCollaboration);
      setDispatchPolicy(nextPolicy);
      setDispatchPreview(nextPreview);
      setDispatchDecisions(nextDispatchDecisions);
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
          <div className="brand-mark">AC</div>
          <div>
            <strong>Agent Company</strong>
            <span>Work OS</span>
          </div>
        </div>

        <nav>
          <button className={view === "queue" ? "nav-active" : ""} onClick={() => setView("queue")}>
            Work Queue <span>{tasks.length}</span>
          </button>
          <button className={view === "agents" ? "nav-active" : ""} onClick={() => setView("agents")}>
            Agent Fleet <span>{agents.filter((a) => a.instance.status === "online").length}</span>
          </button>
        </nav>

        <div className="system-card">
          <span className="status-dot" />
          <div>
            <strong>Local daemon</strong>
            <small>127.0.0.1:8787</small>
          </div>
        </div>
      </aside>

      <main>
        <header className="topbar">
          <div>
            <p className="eyebrow">LOCAL-FIRST CONTROL PLANE</p>
            <h1>{view === "queue" ? "Work Queue" : "Agent Fleet"}</h1>
          </div>
          <div className="mcp-pill">MCP /mcp</div>
        </header>

        {error && <div className="error-banner">{error}</div>}

        {view === "queue" ? (
          <div className="workspace-grid">
            <section className="panel queue-panel">
              <form className="new-task" onSubmit={createTask}>
                <input value={title} onChange={(e) => setTitle(e.target.value)} placeholder="New task…" />
                <input
                  className="priority-input"
                  type="number"
                  value={priority}
                  onChange={(e) => setPriority(Number(e.target.value))}
                  title="Priority"
                />
                <button type="submit">Create</button>
                <button type="button" className="secondary" onClick={() => void dispatchNextTask()} disabled={dispatchBusy}>Dispatch next</button>
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
                      <StateBadge state={task.state} />
                      {dispatchPolicies.some((policy) => policy.task_id === task.id && policy.role === "executor" && policy.enabled) && <span className="badge state-online">dispatch</span>}
                      <span>P{task.priority}</span>
                    </div>
                  </button>
                ))}
                {!tasks.length && <div className="empty">No tasks yet. Create the first work item above.</div>}
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
                    <StateBadge state={selectedTask.state} />
                  </div>
                  <p className="description">{selectedTask.description || "No description."}</p>

                  <div className="detail-columns">
                    <div>
                      <h3>Context</h3>
                      {context ? (
                        <div className="context-card">
                          <div className="section-caption">Revision v{context.version}</div>
                          <strong>{context.goal || "No explicit goal"}</strong>
                          <p>{context.current_summary || context.background || "No summary yet."}</p>
                          <small>{context.created_by_actor_id} · {age(context.created_at)}</small>
                        </div>
                      ) : (
                        <div className="empty compact">No context revision yet.</div>
                      )}

                      <h3>Dispatcher</h3>
                      <div className="dispatch-card">
                        <div className="dispatch-form">
                          <label>
                            <span>Required capabilities</span>
                            <input value={dispatchCapabilities} onChange={(e) => setDispatchCapabilities(e.target.value)} placeholder="code, rust, review" />
                          </label>
                          <label className="dispatch-small">
                            <span>Lease seconds</span>
                            <input type="number" min={30} value={dispatchLease} onChange={(e) => setDispatchLease(Number(e.target.value))} />
                          </label>
                          <label className="dispatch-check">
                            <input type="checkbox" checked={dispatchEnabled} onChange={(e) => setDispatchEnabled(e.target.checked)} />
                            Enabled
                          </label>
                        </div>
                        <div className="dispatch-actions">
                          <button onClick={() => void saveDispatchPolicy()} disabled={dispatchBusy}>Save policy</button>
                          <button className="secondary" onClick={() => void previewDispatch()} disabled={dispatchBusy || !dispatchPolicy}>Preview</button>
                          <button className="secondary" onClick={() => void dispatchSelectedTask()} disabled={dispatchBusy || !dispatchPolicy}>Dispatch</button>
                        </div>
                        {dispatchPolicy ? (
                          <small>executor · heartbeat ≤ {dispatchPolicy.heartbeat_ttl_seconds}s · capacity ≤ {dispatchPolicy.capacity_ttl_seconds}s</small>
                        ) : <div className="empty compact">No executor dispatch policy.</div>}
                        {dispatchPreview && <div className="dispatch-preview">
                          <div className="mini-card-row">
                            <strong>{dispatchPreview.selected_agent_instance_id ? `Selected ${shortId(dispatchPreview.selected_agent_instance_id)}` : "No eligible agent"}</strong>
                            <StateBadge state={dispatchPreview.task_dispatchable ? "ready" : "blocked"} />
                          </div>
                          {dispatchPreview.task_reasons.length > 0 && <p>Task: {dispatchPreview.task_reasons.join(", ")}</p>}
                          {dispatchPreview.candidates.slice(0, 6).map((candidate) => (
                            <div className={`candidate-row ${candidate.eligible ? "candidate-ok" : ""}`} key={candidate.agent_instance_id}>
                              <div>
                                <strong>{candidate.agent_name}</strong>
                                <small>{shortId(candidate.agent_instance_id)} · {candidate.effective_slots} effective slots · {candidate.current_active_assignments} active</small>
                              </div>
                              <span>{candidate.eligible ? "eligible" : candidate.reasons.join(", ")}</span>
                            </div>
                          ))}
                        </div>}
                        {dispatchDecisions.length > 0 && <div className="dispatch-history">
                          <strong>Decision history</strong>
                          {dispatchDecisions.slice(0, 4).map((decision) => (
                            <small key={decision.id}>{decision.outcome} · {decision.selected_agent_instance_id ? shortId(decision.selected_agent_instance_id) : "no agent"} · {age(decision.created_at)}</small>
                          ))}
                        </div>}
                      </div>

                      <h3>Assignments</h3>
                      <div className="stack">
                        {assignments.map((item) => (
                          <div className="mini-card" key={item.id}>
                            <div><strong>{item.role}</strong><StateBadge state={item.status} /></div>
                            <code>{shortId(item.agent_instance_id)}</code>
                            <small>lease → {new Date(item.expires_at).toLocaleTimeString()}</small>
                          </div>
                        ))}
                        {!assignments.length && <div className="empty compact">Unassigned.</div>}
                      </div>

                      <h3>Runs</h3>
                      <div className="stack">
                        {runs.map((run) => (
                          <div className="mini-card" key={run.id}>
                            <div><code>{shortId(run.id)}</code><StateBadge state={run.status} /></div>
                            <small>{run.external_session_ref || "no external session"} · {age(run.started_at)}</small>
                          </div>
                        ))}
                        {!runs.length && <div className="empty compact">No runs yet.</div>}
                      </div>
                    </div>

                    <div>
                      {collaboration && <>
                        <h3>Handoffs</h3>
                        {collaboration.handoffs.map((handoff) => <div className="context-card" key={handoff.id}>
                          <div className="mini-card-row">
                            <strong>{handoff.summary}</strong>
                            <StateBadge state={handoff.status} />
                          </div>
                          <p>Completed: {handoff.completed.join("; ") || "None recorded"}</p>
                          <p>Remaining: {handoff.remaining.join("; ")}</p>
                          <p>Blockers: {handoff.blockers.join("; ") || "None"}</p>
                          <small>
                            Context {shortId(handoff.context_revision_id)}
                            {handoff.accepted_by_run_id ? ` · accepted by run ${shortId(handoff.accepted_by_run_id)}` : ""}
                          </small>
                        </div>)}
                        {!collaboration.handoffs.length && <div className="empty compact">No handoffs.</div>}
                        <h3>Artifacts</h3>
                        {collaboration.artifacts.map((artifact) => <div className="mini-card" key={artifact.id}>
                          <div className="mini-card-row"><strong>{artifact.title}</strong><span className="badge">{artifact.kind}</span></div>
                          <p>{artifact.description}</p><code>{artifact.uri}</code>
                        </div>)}
                        {!collaboration.artifacts.length && <div className="empty compact">No artifacts.</div>}
                        <h3>Decisions</h3>
                        {collaboration.decisions.map((decision) => <div className="mini-card" key={decision.id}>
                          <strong>{decision.title}</strong><p>{decision.rationale}</p>
                        </div>)}
                        {!collaboration.decisions.length && <div className="empty compact">No decisions.</div>}
                        <h3>Discussion</h3>
                        {collaboration.threads.map((thread) => <div className="context-card" key={thread.id}>
                          <strong>{thread.title}</strong>
                          {collaboration.messages.filter((message) => message.thread_id === thread.id).map((message) =>
                            <div className="message-row" key={message.id}>
                              <small>
                                {message.message_type} · {shortId(message.created_by)}
                                {message.recipient_agent_instance_id ? ` → ${shortId(message.recipient_agent_instance_id)}` : ""}
                                {message.recipient_role ? ` (${message.recipient_role})` : ""}
                                {message.reply_to_message_id ? ` · reply to ${shortId(message.reply_to_message_id)}` : ""}
                                {message.requires_response ? " · response required" : ""}
                              </small>
                              <p>{message.body}</p>
                            </div>)}
                        </div>)}
                        {!collaboration.threads.length && <div className="empty compact">No discussions.</div>}
                        <h3>Prerequisites</h3>
                        {collaboration.dependencies.map((dependency) => <div key={dependency.depends_on_task_id}>
                          <button onClick={() => setSelectedId(dependency.depends_on_task_id)}>
                            {tasks.find((task) => task.id === dependency.depends_on_task_id)?.title || shortId(dependency.depends_on_task_id)}
                          </button>
                        </div>)}
                        {!collaboration.dependencies.length && <div className="empty compact">No prerequisites.</div>}
                      </>}
                      <h3>Event Timeline</h3>
                      <div className="timeline">
                        {events.map((event) => (
                          <div className="timeline-item" key={event.id}>
                            <span className="timeline-dot" />
                            <div>
                              <strong>{event.event_type}</strong>
                              <p>{event.actor_type}:{event.actor_id}</p>
                              <small>{new Date(event.created_at).toLocaleString()}</small>
                            </div>
                          </div>
                        ))}
                        {!events.length && <div className="empty compact">No events.</div>}
                      </div>
                    </div>
                  </div>
                </>
              ) : (
                <div className="empty large">Select a task.</div>
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
                  <StateBadge state={agent.status} />
                </div>
                <dl className="fleet-identity">
                  <div><dt>Profile</dt><dd>{profile.name} · {profile.provider || "unknown provider"}{profile.kind ? ` · ${profile.kind}` : ""}</dd></div>
                  <div><dt>Account</dt><dd>{account ? `${account.label} · ${account.provider || "unknown provider"} · ${account.status}` : "Not linked"}</dd></div>
                  <div><dt>Machine</dt><dd>{machine ? `${machine.name} · ${machine.hostname} · ${machine.os} ${machine.arch}` : "Not linked"}</dd></div>
                </dl>
                <div className="capability-list">
                  {agent.capabilities.length ? agent.capabilities.map((cap) => <span key={cap}>{cap}</span>) : <span>general</span>}
                </div>
                <small title={new Date(agent.last_heartbeat_at).toLocaleString()}>Heartbeat {age(agent.last_heartbeat_at)}</small>
                <div className="fleet-capacity">
                  {capacity ? <>
                    <div className="mini-card-row"><strong>Capacity</strong><StateBadge state={capacity.status} /></div>
                    <p>{capacity.available_slots} available slots{capacity.max_concurrency !== null ? ` / ${capacity.max_concurrency} max` : " · max not reported"}</p>
                    <p>{capacity.active_assignments} active assignments · {capacity.active_runs} active runs</p>
                    <small>Quota: {capacity.quota_state ?? "not reported"} · observed {age(capacity.observed_at)}</small>
                  </> : <p>No capacity reported.</p>}
                </div>
              </article>
            ))}
            {!agents.length && <div className="empty large">No agent instances registered yet.</div>}
          </section>
        )}
      </main>
    </div>
  );
}
