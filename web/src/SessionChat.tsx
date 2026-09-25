import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api } from "./api";
import { formatAge } from "./i18n";
import type { Locale } from "./i18n";

export type ChatAgent = {
  id: string;
  name: string;
  status: string;
  account_id?: string | null;
  account_email?: string | null;
};

export type ChatProject = {
  id: string;
  name: string;
};

export type ChatTask = {
  id: string;
  project_id?: string | null;
  title: string;
};

type SessionSummary = {
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
  last_message_author_type?: string | null;
  last_message_at?: string | null;
  created_at: string;
  updated_at: string;
};

type Session = {
  id: string;
  agent_instance_id: string;
  project_id?: string | null;
  task_id?: string | null;
  title: string;
  status: string;
  created_at: string;
  updated_at: string;
};

type SessionMessage = {
  id: string;
  session_id: string;
  author_type: "human" | "agent" | "system";
  author_agent_instance_id?: string | null;
  body: string;
  client_message_id?: string | null;
  recalled_at?: string | null;
  status: "queued" | "delivered";
  delivery_status?: "queued" | "claimed" | "delivered" | null;
  created_at: string;
};

type SessionHistory = {
  session: Session;
  messages: SessionMessage[];
  has_more: boolean;
  next_before?: string | null;
};

type SessionRuntimeAttempt = {
  id: string;
  session_id: string;
  agent_instance_id: string;
  account_id?: string | null;
  launch_profile_id: string;
  adapter: string;
  status: "queued" | "running" | "completed" | "failed";
  model?: string | null;
  reasoning_effort?: string | null;
  provider_session_ref?: string | null;
  pid?: number | null;
  exit_code?: number | null;
  error?: string | null;
  created_at: string;
  started_at?: string | null;
  ended_at?: string | null;
};

type RuntimeModelOption = {
  id: string;
  label: string;
  reasoning_efforts: string[];
  default_reasoning_effort?: string;
};

type SessionRuntimeOptions = {
  available: boolean;
  reason?: string;
  adapter?: string;
  launch_profile_id?: string;
  models: RuntimeModelOption[];
  reasoning_efforts: string[];
  selected_model: string;
  selected_reasoning_effort: string;
};

type SessionSummaryRevision = {
  id: string;
  session_id: string;
  previous_revision_id?: string | null;
  covers_until_message_id?: string | null;
  goal: string;
  current_state: string;
  important_findings: unknown;
  decisions: unknown;
  blockers: unknown;
  unresolved_questions: unknown;
  next_steps: unknown;
  deterministic_facts: unknown;
  created_by: string;
  created_at: string;
};

type Props = {
  agents: ChatAgent[];
  projects: ChatProject[];
  tasks: ChatTask[];
  locale: Locale;
  initialAgentId?: string | null;
  initialSessionId?: string | null;
  initialTaskId?: string | null;
  onInitialAgentHandled?: () => void;
  onInitialSessionHandled?: () => void;
  onInitialTaskHandled?: () => void;
};

function mergeMessages(current: SessionMessage[], incoming: SessionMessage[]) {
  const agentReplied = incoming.some((message) => message.author_type === "agent");
  const normalizedCurrent = agentReplied
    ? current.map((message) =>
        message.author_type === "human" && message.status === "queued"
          ? { ...message, status: "delivered" as const }
          : message,
      )
    : current;
  const byId = new Map(normalizedCurrent.map((message) => [message.id, message]));
  for (const message of incoming) byId.set(message.id, message);
  return [...byId.values()].sort((a, b) =>
    a.created_at === b.created_at ? a.id.localeCompare(b.id) : a.created_at.localeCompare(b.created_at),
  );
}

export default function SessionChat({
  agents,
  projects,
  tasks,
  locale,
  initialAgentId,
  initialSessionId,
  initialTaskId,
  onInitialAgentHandled,
  onInitialSessionHandled,
  onInitialTaskHandled,
}: Props) {
  const zh = locale === "zh-CN";
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [historyCache, setHistoryCache] = useState<Record<string, SessionHistory>>({});
  const [summaryCache, setSummaryCache] = useState<Record<string, SessionSummaryRevision | null>>({});
  const [runtimeCache, setRuntimeCache] = useState<Record<string, SessionRuntimeAttempt | null>>({});
  const [runtimeOptionsCache, setRuntimeOptionsCache] = useState<Record<string, SessionRuntimeOptions | null>>({});
  const historyCacheRef = useRef<Record<string, SessionHistory>>({});
  const sendInFlightRef = useRef(false);
  const [loadingId, setLoadingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [newAgentId, setNewAgentId] = useState("");
  const [newScope, setNewScope] = useState<"general" | "project" | "task">("general");
  const [newProjectId, setNewProjectId] = useState("");
  const [newTaskId, setNewTaskId] = useState("");
  const [scopeDraft, setScopeDraft] = useState<"general" | "project" | "task">("general");
  const [scopeProjectId, setScopeProjectId] = useState("");
  const [scopeTaskId, setScopeTaskId] = useState("");
  const [showNew, setShowNew] = useState(false);
  const [busy, setBusy] = useState(false);
  const [recallBusyId, setRecallBusyId] = useState<string | null>(null);
  const [runtimeBusy, setRuntimeBusy] = useState(false);
  const [runtimeModel, setRuntimeModel] = useState("");
  const [runtimeEffort, setRuntimeEffort] = useState("");
  const [error, setError] = useState<string | null>(null);

  const selected = useMemo(
    () => sessions.find((session) => session.id === selectedId) ?? null,
    [sessions, selectedId],
  );
  const history = selectedId ? historyCache[selectedId] : undefined;
  const summary = selectedId ? summaryCache[selectedId] : undefined;
  const runtime = selectedId ? runtimeCache[selectedId] : undefined;
  const runtimeOptions = selectedId ? runtimeOptionsCache[selectedId] : undefined;
  const selectedAgent = selected
    ? agents.find((agent) => agent.id === selected.agent_instance_id) ?? null
    : null;
  const runtimeActive = runtime?.status === "queued" || runtime?.status === "running";
  const selectedModelOption = runtimeOptions?.models.find((model) => model.id === runtimeModel);
  const runtimeEffortOptions = selectedModelOption?.reasoning_efforts?.length
    ? selectedModelOption.reasoning_efforts
    : runtimeOptions?.reasoning_efforts ?? [];
  const displayAgentName = useCallback(
    (agentId: string, fallback: string) => agents.find((agent) => agent.id === agentId)?.name || fallback,
    [agents],
  );
  const displaySessionTitle = useCallback(
    (session: SessionSummary) => {
      const friendlyName = displayAgentName(session.agent_instance_id, session.agent_name);
      return friendlyName === session.agent_name
        ? session.title
        : session.title.replace(session.agent_name, friendlyName);
    },
    [displayAgentName],
  );

  const scopeLabel = useCallback((session: SessionSummary) => {
    if (session.task_id) {
      const project = session.project_name ? `${session.project_name} / ` : "";
      return `${project}${session.task_title || session.task_id}`;
    }
    if (session.project_id) return session.project_name || session.project_id;
    return zh ? "通用会话" : "General";
  }, [zh]);

  const unclassifiedProjectKey = "__unclassified__";
  const hasUnclassifiedTasks = tasks.some((task) => !task.project_id);
  const newTaskOptions = useMemo(
    () => tasks.filter((task) =>
      newProjectId === unclassifiedProjectKey
        ? !task.project_id
        : !!newProjectId && task.project_id === newProjectId,
    ),
    [tasks, newProjectId],
  );
  const scopeTaskOptions = useMemo(
    () => tasks.filter((task) =>
      scopeProjectId === unclassifiedProjectKey
        ? !task.project_id
        : !!scopeProjectId && task.project_id === scopeProjectId,
    ),
    [tasks, scopeProjectId],
  );

  useEffect(() => {
    if (!selected) return;
    if (selected.task_id) {
      setScopeDraft("task");
      setScopeProjectId(selected.project_id || unclassifiedProjectKey);
      setScopeTaskId(selected.task_id);
      return;
    }
    if (selected.project_id) {
      setScopeDraft("project");
      setScopeProjectId(selected.project_id);
      setScopeTaskId("");
      return;
    }
    setScopeDraft("general");
    setScopeProjectId("");
    setScopeTaskId("");
  }, [selected?.id, selected?.project_id, selected?.task_id]);

  useEffect(() => {
    historyCacheRef.current = historyCache;
  }, [historyCache]);

  const refreshSummaries = useCallback(async () => {
    try {
      const next = await api<SessionSummary[]>("/api/sessions");
      setSessions(next.filter((session) => session.status === "open"));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, []);

  useEffect(() => {
    void refreshSummaries();
    const timer = window.setInterval(() => void refreshSummaries(), 5000);
    return () => window.clearInterval(timer);
  }, [refreshSummaries]);

  useEffect(() => {
    if (!initialAgentId) return;
    setNewAgentId(initialAgentId);
    setShowNew(true);
    onInitialAgentHandled?.();
  }, [initialAgentId, onInitialAgentHandled]);

  useEffect(() => {
    if (!initialTaskId) return;
    const task = tasks.find((item) => item.id === initialTaskId);
    if (!task) return;
    setNewScope("task");
    setNewProjectId(task.project_id || unclassifiedProjectKey);
    setNewTaskId(initialTaskId);
    setShowNew(true);
    onInitialTaskHandled?.();
  }, [initialTaskId, onInitialTaskHandled, tasks]);

  const loadSession = useCallback(async (id: string, force = false) => {
    if (!force && historyCacheRef.current[id]) return;
    setLoadingId(id);
    try {
      const [loaded, loadedSummary, loadedRuntime, loadedOptions] = await Promise.all([
        api<SessionHistory>(`/api/sessions/${id}/messages?limit=80`),
        api<SessionSummaryRevision | null>(`/api/sessions/${id}/summary`).catch(() => null),
        api<SessionRuntimeAttempt | null>(`/api/sessions/${id}/runtime`).catch(() => null),
        api<SessionRuntimeOptions>(`/api/sessions/${id}/runtime/options`).catch(() => null),
      ]);
      setHistoryCache((current) => ({ ...current, [id]: loaded }));
      setSummaryCache((current) => ({ ...current, [id]: loadedSummary }));
      setRuntimeCache((current) => ({ ...current, [id]: loadedRuntime }));
      setRuntimeOptionsCache((current) => ({ ...current, [id]: loadedOptions }));
      setRuntimeModel(loadedRuntime?.model ?? loadedOptions?.selected_model ?? "");
      setRuntimeEffort(loadedRuntime?.reasoning_effort ?? loadedOptions?.selected_reasoning_effort ?? "");
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingId((current) => (current === id ? null : current));
    }
  }, []);

  useEffect(() => {
    if (!initialSessionId) return;
    setSelectedId(initialSessionId);
    void loadSession(initialSessionId);
    onInitialSessionHandled?.();
  }, [initialSessionId, loadSession, onInitialSessionHandled]);

  async function selectSession(id: string) {
    setSelectedId(id);
    const cachedRuntime = runtimeCache[id];
    const cachedOptions = runtimeOptionsCache[id];
    if (cachedRuntime !== undefined || cachedOptions !== undefined) {
      setRuntimeModel(cachedRuntime?.model ?? cachedOptions?.selected_model ?? "");
      setRuntimeEffort(cachedRuntime?.reasoning_effort ?? cachedOptions?.selected_reasoning_effort ?? "");
    }
    await loadSession(id);
  }

  useEffect(() => {
    if (!selectedId) return;
    const timer = window.setInterval(() => {
      const current = historyCacheRef.current[selectedId];
      if (!current) return;
      // Refresh only the selected session's recent window. Merging keeps any
      // explicitly loaded older pages while also updating delivery/reply states.
      const path = `/api/sessions/${selectedId}/messages?limit=80`;
      void api<SessionHistory>(path)
        .then((next) => {
          setHistoryCache((all) => {
            const previous = all[selectedId];
            if (!previous) return { ...all, [selectedId]: next };
            return {
              ...all,
              [selectedId]: {
                ...previous,
                session: next.session,
                messages: mergeMessages(previous.messages, next.messages),
              },
            };
          });
        })
        .catch(() => {});
      void api<SessionSummaryRevision | null>(`/api/sessions/${selectedId}/summary`)
        .then((next) => setSummaryCache((all) => ({ ...all, [selectedId]: next })))
        .catch(() => {});
      void api<SessionRuntimeAttempt | null>(`/api/sessions/${selectedId}/runtime`)
        .then((next) => setRuntimeCache((all) => ({ ...all, [selectedId]: next })))
        .catch(() => {});
    }, 2500);
    return () => window.clearInterval(timer);
  }, [selectedId]);

  async function createSession(event: FormEvent) {
    event.preventDefault();
    if (!newAgentId) return;
    const agent = agents.find((item) => item.id === newAgentId);
    if (!agent) return;
    setBusy(true);
    try {
      const task = newScope === "task" ? tasks.find((item) => item.id === newTaskId) : null;
      const project = newScope === "project" ? projects.find((item) => item.id === newProjectId) : null;
      const created = await api<Session>("/api/sessions", {
        method: "POST",
        body: JSON.stringify({
          agent_instance_id: agent.id,
          project_id: newScope === "project" ? newProjectId || null : null,
          task_id: newScope === "task" ? newTaskId || null : null,
          title: task
            ? `${task.title} · ${agent.name}`
            : project
              ? `${project.name} · ${agent.name}`
              : zh ? `与 ${agent.name} 的会话` : `Session with ${agent.name}`,
        }),
      });
      setHistoryCache((current) => ({
        ...current,
        [created.id]: { session: created, messages: [], has_more: false, next_before: null },
      }));
      setSummaryCache((current) => ({ ...current, [created.id]: null }));
      setSelectedId(created.id);
      setShowNew(false);
      await refreshSummaries();
      await loadSession(created.id, true);
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function saveScope() {
    if (!selectedId) return;
    if (scopeDraft === "project" && (!scopeProjectId || scopeProjectId === unclassifiedProjectKey)) return;
    if (scopeDraft === "task" && !scopeTaskId) return;
    setBusy(true);
    try {
      const updated = await api<Session>(`/api/sessions/${selectedId}/scope`, {
        method: "POST",
        body: JSON.stringify({
          project_id: scopeDraft === "project" ? scopeProjectId : null,
          task_id: scopeDraft === "task" ? scopeTaskId : null,
        }),
      });
      setHistoryCache((current) => {
        const cached = current[selectedId];
        return cached ? { ...current, [selectedId]: { ...cached, session: updated } } : current;
      });
      await refreshSummaries();
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  }

  async function sendMessage(event: FormEvent) {
    event.preventDefault();
    if (sendInFlightRef.current || !selectedId || !draft.trim()) return;
    sendInFlightRef.current = true;
    const body = draft.trim();
    const clientMessageId = crypto.randomUUID();
    setDraft("");
    setBusy(true);
    try {
      const message = await api<SessionMessage>(`/api/sessions/${selectedId}/messages`, {
        method: "POST",
        body: JSON.stringify({ body, client_message_id: clientMessageId }),
      });
      setHistoryCache((current) => {
        const cached = current[selectedId];
        if (!cached) return current;
        return {
          ...current,
          [selectedId]: { ...cached, messages: mergeMessages(cached.messages, [message]) },
        };
      });
      await refreshSummaries();
      setError(null);
    } catch (e) {
      setDraft(body);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      sendInFlightRef.current = false;
      setBusy(false);
    }
  }

  async function recallMessage(messageId: string) {
    if (!selectedId || recallBusyId) return;
    setRecallBusyId(messageId);
    try {
      const message = await api<SessionMessage>(
        `/api/sessions/${selectedId}/messages/${messageId}/recall`,
        { method: "POST" },
      );
      setHistoryCache((current) => {
        const cached = current[selectedId];
        if (!cached) return current;
        return {
          ...current,
          [selectedId]: {
            ...cached,
            messages: cached.messages.map((item) => (item.id === message.id ? message : item)),
          },
        };
      });
      await refreshSummaries();
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setRecallBusyId(null);
    }
  }

  async function startAgentRuntime() {
    if (!selectedId || runtimeBusy) return;
    setRuntimeBusy(true);
    try {
      const started = await api<SessionRuntimeAttempt>(`/api/sessions/${selectedId}/runtime/start`, {
        method: "POST",
        body: JSON.stringify({
          model: runtimeModel,
          reasoning_effort: runtimeEffort,
        }),
      });
      setRuntimeCache((current) => ({ ...current, [selectedId]: started }));
      setRuntimeModel(started.model ?? "");
      setRuntimeEffort(started.reasoning_effort ?? "");
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setRuntimeBusy(false);
    }
  }

  async function loadOlder() {
    if (!selectedId || !history?.has_more || !history.next_before) return;
    setLoadingId(selectedId);
    try {
      const older = await api<SessionHistory>(
        `/api/sessions/${selectedId}/messages?before=${encodeURIComponent(history.next_before)}&limit=80`,
      );
      setHistoryCache((current) => {
        const cached = current[selectedId];
        if (!cached) return current;
        return {
          ...current,
          [selectedId]: {
            ...cached,
            messages: mergeMessages(older.messages, cached.messages),
            has_more: older.has_more,
            next_before: older.next_before,
          },
        };
      });
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingId(null);
    }
  }

  return (
    <div className="chat-workspace">
      <section className="panel session-list-panel">
        <div className="session-list-head">
          <div>
            <strong>{zh ? "会话" : "Sessions"}</strong>
            <small>{zh ? "只加载会话摘要" : "Summary-only at startup"}</small>
          </div>
          <button type="button" onClick={() => setShowNew((value) => !value)}>
            {showNew ? (zh ? "取消" : "Cancel") : (zh ? "新会话" : "New")}
          </button>
        </div>

        {showNew && (
          <form className="new-session" onSubmit={createSession}>
            <select value={newAgentId} onChange={(event) => setNewAgentId(event.target.value)}>
              <option value="">{zh ? "选择 Agent…" : "Select agent…"}</option>
              {agents.map((agent) => (
                <option key={agent.id} value={agent.id}>
                  {agent.name} · {agent.status}
                </option>
              ))}
            </select>
            <select
              value={newScope}
              onChange={(event) => {
                const value = event.target.value as "general" | "project" | "task";
                setNewScope(value);
                setNewProjectId("");
                setNewTaskId("");
              }}
            >
              <option value="general">{zh ? "通用会话" : "General session"}</option>
              <option value="project">{zh ? "归属项目" : "Project-scoped"}</option>
              <option value="task">{zh ? "归属任务" : "Task-scoped"}</option>
            </select>
            {(newScope === "project" || newScope === "task") && (
              <select
                value={newProjectId}
                onChange={(event) => {
                  setNewProjectId(event.target.value);
                  setNewTaskId("");
                }}
              >
                <option value="">{zh ? "选择项目…" : "Select project…"}</option>
                {projects.map((project) => <option key={project.id} value={project.id}>{project.name}</option>)}
                {newScope === "task" && hasUnclassifiedTasks && (
                  <option value={unclassifiedProjectKey}>{zh ? "未归项目" : "No project"}</option>
                )}
              </select>
            )}
            {newScope === "task" && (
              <select
                value={newTaskId}
                disabled={!newProjectId}
                onChange={(event) => setNewTaskId(event.target.value)}
              >
                <option value="">{zh ? "选择任务…" : "Select task…"}</option>
                {newTaskOptions.map((task) => (
                  <option key={task.id} value={task.id}>{task.title}</option>
                ))}
              </select>
            )}
            <button
              disabled={
                !newAgentId ||
                busy ||
                (newScope === "project" && !newProjectId) ||
                (newScope === "task" && !newTaskId)
              }
            >
              {zh ? "开始" : "Start"}
            </button>
          </form>
        )}

        <div className="session-list">
          {sessions.map((session) => (
            <button
              type="button"
              className={`session-row ${selectedId === session.id ? "selected" : ""}`}
              key={session.id}
              onClick={() => void selectSession(session.id)}
            >
              <div className="session-avatar">{displayAgentName(session.agent_instance_id, session.agent_name).slice(0, 2).toUpperCase()}</div>
              <div className="session-row-body">
                <div className="session-row-title">
                  <strong>{displaySessionTitle(session)}</strong>
                  <small>{formatAge(locale, session.last_message_at || session.updated_at)}</small>
                </div>
                <span>{displayAgentName(session.agent_instance_id, session.agent_name)} · {scopeLabel(session)}</span>
                <p>{session.last_message_preview || (zh ? "还没有消息" : "No messages yet")}</p>
              </div>
              {session.queued_count > 0 && (
                <span
                  className="queued-count"
                  title={
                    zh
                      ? `${session.queued_count} 条待回复，其中 ${session.undelivered_count} 条尚未投递`
                      : `${session.queued_count} awaiting reply; ${session.undelivered_count} not yet delivered`
                  }
                >
                  {session.queued_count}
                </span>
              )}
            </button>
          ))}
          {!sessions.length && <div className="empty">{zh ? "还没有会话。" : "No sessions yet."}</div>}
        </div>
      </section>

      <section className="panel chat-panel">
        {error && <div className="error-banner chat-error">{error}</div>}
        {!selected ? (
          <div className="chat-empty">
            <strong>{zh ? "选择一个会话" : "Select a session"}</strong>
            <p>{zh ? "历史记录只有在你点开会话后才会加载进当前页面缓存。" : "History is fetched into this page cache only after you select a session."}</p>
          </div>
        ) : (
          <>
            <header className="chat-header">
              <div className="session-avatar large">{displayAgentName(selected.agent_instance_id, selected.agent_name).slice(0, 2).toUpperCase()}</div>
              <div>
                <h2>{displaySessionTitle(selected)}</h2>
                <span>
                  {displayAgentName(selected.agent_instance_id, selected.agent_name)}
                  {" · "}
                  {selectedAgent?.account_email || (zh ? "未记录邮箱" : "Email not recorded")}
                </span>
                <div className="session-scope-editor">
                  <span>{zh ? "归属" : "Scope"}</span>
                  <select
                    value={scopeDraft}
                    disabled={busy}
                    onChange={(event) => {
                      const value = event.target.value as "general" | "project" | "task";
                      setScopeDraft(value);
                      if (value === "general") {
                        setScopeProjectId("");
                        setScopeTaskId("");
                      } else if (value === "project") {
                        if (scopeProjectId === unclassifiedProjectKey) setScopeProjectId("");
                        setScopeTaskId("");
                      } else {
                        if (!scopeProjectId && selected.project_id) {
                          setScopeProjectId(selected.project_id);
                        } else if (!scopeProjectId && selected.task_id && !selected.project_id) {
                          setScopeProjectId(unclassifiedProjectKey);
                        }
                      }
                    }}
                  >
                    <option value="general">{zh ? "通用会话" : "General"}</option>
                    <option value="project">{zh ? "项目" : "Project"}</option>
                    <option value="task">{zh ? "任务" : "Task"}</option>
                  </select>
                  {(scopeDraft === "project" || scopeDraft === "task") && (
                    <select
                      value={scopeProjectId}
                      disabled={busy}
                      onChange={(event) => {
                        setScopeProjectId(event.target.value);
                        setScopeTaskId("");
                      }}
                    >
                      <option value="">{zh ? "选择项目…" : "Select project…"}</option>
                      {projects.map((project) => (
                        <option key={project.id} value={project.id}>{project.name}</option>
                      ))}
                      {scopeDraft === "task" && hasUnclassifiedTasks && (
                        <option value={unclassifiedProjectKey}>{zh ? "未归项目" : "No project"}</option>
                      )}
                    </select>
                  )}
                  {scopeDraft === "task" && (
                    <select
                      value={scopeTaskId}
                      disabled={busy || !scopeProjectId}
                      onChange={(event) => setScopeTaskId(event.target.value)}
                    >
                      <option value="">{zh ? "选择任务…" : "Select task…"}</option>
                      {scopeTaskOptions.map((task) => (
                        <option key={task.id} value={task.id}>{task.title}</option>
                      ))}
                    </select>
                  )}
                  <button
                    type="button"
                    className="session-scope-save"
                    disabled={
                      busy ||
                      (scopeDraft === "project" && (!scopeProjectId || scopeProjectId === unclassifiedProjectKey)) ||
                      (scopeDraft === "task" && !scopeTaskId)
                    }
                    onClick={() => void saveScope()}
                  >
                    {busy ? (zh ? "保存中…" : "Saving…") : (zh ? "保存归属" : "Save scope")}
                  </button>
                </div>
              </div>
              <div className="session-runtime-controls">
                <label className="session-runtime-select">
                  <span>{zh ? "模型" : "Model"}</span>
                  <select
                    value={runtimeModel}
                    disabled={runtimeBusy || runtimeActive || !runtimeOptions?.available}
                    onChange={(event) => {
                      const nextModel = event.target.value;
                      setRuntimeModel(nextModel);
                      const modelOption = runtimeOptions?.models.find((model) => model.id === nextModel);
                      if (
                        runtimeEffort &&
                        modelOption?.reasoning_efforts?.length &&
                        !modelOption.reasoning_efforts.includes(runtimeEffort)
                      ) {
                        setRuntimeEffort(modelOption.default_reasoning_effort || "");
                      }
                    }}
                  >
                    <option value="">{zh ? "配置默认" : "Config default"}</option>
                    {runtimeOptions?.models.map((model) => (
                      <option key={model.id} value={model.id}>{model.label}</option>
                    ))}
                  </select>
                </label>
                <label className="session-runtime-select">
                  <span>{zh ? "思考" : "Reasoning"}</span>
                  <select
                    value={runtimeEffort}
                    disabled={runtimeBusy || runtimeActive || !runtimeOptions?.available}
                    onChange={(event) => setRuntimeEffort(event.target.value)}
                  >
                    <option value="">{zh ? "配置默认" : "Config default"}</option>
                    {runtimeEffortOptions.map((effort) => (
                      <option key={effort} value={effort}>{effort}</option>
                    ))}
                  </select>
                </label>
                <button
                  type="button"
                  className="session-runtime-start"
                  disabled={runtimeBusy || runtimeActive || !runtimeOptions?.available}
                  title={runtime?.error || runtimeOptions?.reason || undefined}
                  onClick={() => void startAgentRuntime()}
                >
                  {runtimeBusy
                    ? (zh ? "启动中…" : "Starting…")
                    : runtimeActive
                      ? (zh ? "Agent 运行中" : "Agent running")
                      : runtime?.provider_session_ref
                        ? (zh ? "继续 Agent" : "Resume Agent")
                        : (zh ? "启动 Agent" : "Start Agent")}
                </button>
                {selected.queued_count > 0 && (
                  <span className="delivery-note">
                    {selected.undelivered_count > 0
                      ? (zh ? `${selected.undelivered_count} 条等待投递` : `${selected.undelivered_count} awaiting delivery`)
                      : (zh ? "已投递，等待回复" : "Delivered, awaiting reply")}
                  </span>
                )}
              </div>
            </header>

            <details className="session-summary-card">
              <summary>
                {zh ? "结构化摘要" : "Structured summary"}
                {summary?.current_state ? ` · ${summary.current_state}` : ""}
              </summary>
              {summary ? (
                <div className="session-summary-body">
                  <strong>{summary.goal || (zh ? "未记录目标" : "No recorded goal")}</strong>
                  <p>{summary.current_state || (zh ? "暂无当前状态" : "No current state")}</p>
                  <div className="summary-grid">
                    <span>{zh ? "重要发现" : "Findings"}<pre>{JSON.stringify(summary.important_findings, null, 2)}</pre></span>
                    <span>{zh ? "决策" : "Decisions"}<pre>{JSON.stringify(summary.decisions, null, 2)}</pre></span>
                    <span>{zh ? "阻塞项" : "Blockers"}<pre>{JSON.stringify(summary.blockers, null, 2)}</pre></span>
                    <span>{zh ? "下一步" : "Next steps"}<pre>{JSON.stringify(summary.next_steps, null, 2)}</pre></span>
                  </div>
                  <small>{summary.created_by} · {formatAge(locale, summary.created_at)}</small>
                </div>
              ) : (
                <p className="summary-empty">{zh ? "这个会话还没有结构化摘要。" : "This session has no structured summary yet."}</p>
              )}
            </details>

            <div className="message-scroll">
              {history?.has_more && (
                <button className="load-older" type="button" onClick={() => void loadOlder()} disabled={loadingId === selected.id}>
                  {zh ? "加载更早消息" : "Load older messages"}
                </button>
              )}
              {loadingId === selected.id && !history && <div className="empty">{zh ? "加载历史…" : "Loading history…"}</div>}
              {history?.messages.map((message) => (
                <div className={`chat-message ${message.author_type}`} key={message.id}>
                  <div className="message-bubble">
                    <p className={message.recalled_at ? "recalled-message" : ""}>
                      {message.recalled_at ? (zh ? "消息已撤回" : "Message recalled") : message.body}
                    </p>
                    <div className="message-meta">
                      <small>
                        {message.author_type === "human" ? (zh ? "你" : "You") : displayAgentName(selected.agent_instance_id, selected.agent_name)}
                        {" · "}
                        {new Date(message.created_at).toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" })}
                        {message.recalled_at
                          ? (zh ? " · 已撤回" : " · recalled")
                          : message.author_type === "human" && message.status === "queued"
                            ? message.delivery_status === "delivered"
                              ? (zh ? " · 已投递" : " · delivered")
                              : (zh ? " · 等待投递" : " · awaiting delivery")
                            : ""}
                      </small>
                      {message.author_type === "human" &&
                        !message.recalled_at &&
                        message.status === "queued" &&
                        message.delivery_status === "queued" && (
                          <button
                            type="button"
                            className="message-recall"
                            disabled={recallBusyId === message.id}
                            onClick={() => void recallMessage(message.id)}
                          >
                            {recallBusyId === message.id
                              ? (zh ? "撤回中…" : "Recalling…")
                              : (zh ? "撤回" : "Recall")}
                          </button>
                        )}
                    </div>
                  </div>
                </div>
              ))}
              {history && !history.messages.length && <div className="empty">{zh ? "发第一条消息。" : "Send the first message."}</div>}
            </div>

            <form className="chat-composer" onSubmit={sendMessage}>
              <textarea
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                placeholder={zh ? "给这个 Agent 发消息…" : "Message this agent…"}
                rows={2}
                onKeyDown={(event) => {
                  if (
                    event.key === "Enter" &&
                    !event.shiftKey &&
                    !event.repeat &&
                    !event.nativeEvent.isComposing
                  ) {
                    event.preventDefault();
                    if (!sendInFlightRef.current) event.currentTarget.form?.requestSubmit();
                  }
                }}
              />
              <button disabled={!draft.trim() || busy}>{zh ? "发送" : "Send"}</button>
            </form>
          </>
        )}
      </section>
    </div>
  );
}
