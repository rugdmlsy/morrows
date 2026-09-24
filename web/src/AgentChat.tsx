import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { FormEvent } from "react";
import { api } from "./api";
import { formatAge } from "./i18n";
import type { Locale } from "./i18n";

export type ChatAgent = {
  id: string;
  name: string;
  status: string;
};

type ConversationSummary = {
  id: string;
  agent_instance_id: string;
  agent_name: string;
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

type Conversation = {
  id: string;
  agent_instance_id: string;
  title: string;
  status: string;
  created_at: string;
  updated_at: string;
};

type ConversationMessage = {
  id: string;
  conversation_id: string;
  author_type: "human" | "agent" | "system";
  author_agent_instance_id?: string | null;
  body: string;
  status: "queued" | "delivered";
  delivery_status?: "queued" | "claimed" | "delivered" | null;
  created_at: string;
};

type ConversationHistory = {
  conversation: Conversation;
  messages: ConversationMessage[];
  has_more: boolean;
  next_before?: string | null;
};

type ConversationSummaryRevision = {
  id: string;
  conversation_id: string;
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
  locale: Locale;
  initialAgentId?: string | null;
  onInitialAgentHandled?: () => void;
};

function mergeMessages(current: ConversationMessage[], incoming: ConversationMessage[]) {
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

export default function AgentChat({ agents, locale, initialAgentId, onInitialAgentHandled }: Props) {
  const zh = locale === "zh-CN";
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [historyCache, setHistoryCache] = useState<Record<string, ConversationHistory>>({});
  const [summaryCache, setSummaryCache] = useState<Record<string, ConversationSummaryRevision | null>>({});
  const historyCacheRef = useRef<Record<string, ConversationHistory>>({});
  const [loadingId, setLoadingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [newAgentId, setNewAgentId] = useState("");
  const [showNew, setShowNew] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selected = useMemo(
    () => conversations.find((conversation) => conversation.id === selectedId) ?? null,
    [conversations, selectedId],
  );
  const history = selectedId ? historyCache[selectedId] : undefined;
  const summary = selectedId ? summaryCache[selectedId] : undefined;

  useEffect(() => {
    historyCacheRef.current = historyCache;
  }, [historyCache]);

  const refreshSummaries = useCallback(async () => {
    try {
      setConversations(await api<ConversationSummary[]>("/api/conversations"));
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

  const loadConversation = useCallback(async (id: string, force = false) => {
    if (!force && historyCacheRef.current[id]) return;
    setLoadingId(id);
    try {
      const [loaded, loadedSummary] = await Promise.all([
        api<ConversationHistory>(`/api/conversations/${id}/messages?limit=80`),
        api<ConversationSummaryRevision | null>(`/api/conversations/${id}/summary`).catch(() => null),
      ]);
      setHistoryCache((current) => ({ ...current, [id]: loaded }));
      setSummaryCache((current) => ({ ...current, [id]: loadedSummary }));
      setError(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoadingId((current) => (current === id ? null : current));
    }
  }, []);

  async function selectConversation(id: string) {
    setSelectedId(id);
    await loadConversation(id);
  }

  useEffect(() => {
    if (!selectedId) return;
    const timer = window.setInterval(() => {
      const current = historyCacheRef.current[selectedId];
      if (!current) return;
      // Refresh only the selected conversation's recent window. Merging keeps any
      // explicitly loaded older pages while also updating delivery/reply states.
      const path = `/api/conversations/${selectedId}/messages?limit=80`;
      void api<ConversationHistory>(path)
        .then((next) => {
          setHistoryCache((all) => {
            const previous = all[selectedId];
            if (!previous) return { ...all, [selectedId]: next };
            return {
              ...all,
              [selectedId]: {
                ...previous,
                conversation: next.conversation,
                messages: mergeMessages(previous.messages, next.messages),
              },
            };
          });
        })
        .catch(() => {});
      void api<ConversationSummaryRevision | null>(`/api/conversations/${selectedId}/summary`)
        .then((next) => setSummaryCache((all) => ({ ...all, [selectedId]: next })))
        .catch(() => {});
    }, 2500);
    return () => window.clearInterval(timer);
  }, [selectedId]);

  async function createConversation(event: FormEvent) {
    event.preventDefault();
    if (!newAgentId) return;
    const agent = agents.find((item) => item.id === newAgentId);
    if (!agent) return;
    setBusy(true);
    try {
      const created = await api<Conversation>("/api/conversations", {
        method: "POST",
        body: JSON.stringify({
          agent_instance_id: agent.id,
          title: zh ? `与 ${agent.name} 的对话` : `Conversation with ${agent.name}`,
        }),
      });
      setHistoryCache((current) => ({
        ...current,
        [created.id]: { conversation: created, messages: [], has_more: false, next_before: null },
      }));
      setSummaryCache((current) => ({ ...current, [created.id]: null }));
      setSelectedId(created.id);
      setShowNew(false);
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
    if (!selectedId || !draft.trim()) return;
    const body = draft.trim();
    setDraft("");
    setBusy(true);
    try {
      const message = await api<ConversationMessage>(`/api/conversations/${selectedId}/messages`, {
        method: "POST",
        body: JSON.stringify({ body }),
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
      setBusy(false);
    }
  }

  async function loadOlder() {
    if (!selectedId || !history?.has_more || !history.next_before) return;
    setLoadingId(selectedId);
    try {
      const older = await api<ConversationHistory>(
        `/api/conversations/${selectedId}/messages?before=${encodeURIComponent(history.next_before)}&limit=80`,
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
      <section className="panel conversation-list-panel">
        <div className="conversation-list-head">
          <div>
            <strong>{zh ? "对话" : "Conversations"}</strong>
            <small>{zh ? "只加载会话摘要" : "Summary-only at startup"}</small>
          </div>
          <button type="button" onClick={() => setShowNew((value) => !value)}>
            {showNew ? (zh ? "取消" : "Cancel") : (zh ? "新对话" : "New")}
          </button>
        </div>

        {showNew && (
          <form className="new-conversation" onSubmit={createConversation}>
            <select value={newAgentId} onChange={(event) => setNewAgentId(event.target.value)}>
              <option value="">{zh ? "选择 Agent…" : "Select agent…"}</option>
              {agents.map((agent) => (
                <option key={agent.id} value={agent.id}>
                  {agent.name} · {agent.status}
                </option>
              ))}
            </select>
            <button disabled={!newAgentId || busy}>{zh ? "开始" : "Start"}</button>
          </form>
        )}

        <div className="conversation-list">
          {conversations.map((conversation) => (
            <button
              type="button"
              className={`conversation-row ${selectedId === conversation.id ? "selected" : ""}`}
              key={conversation.id}
              onClick={() => void selectConversation(conversation.id)}
            >
              <div className="conversation-avatar">{conversation.agent_name.slice(0, 2).toUpperCase()}</div>
              <div className="conversation-row-body">
                <div className="conversation-row-title">
                  <strong>{conversation.title}</strong>
                  <small>{formatAge(locale, conversation.last_message_at || conversation.updated_at)}</small>
                </div>
                <span>{conversation.agent_name}</span>
                <p>{conversation.last_message_preview || (zh ? "还没有消息" : "No messages yet")}</p>
              </div>
              {conversation.queued_count > 0 && (
                <span
                  className="queued-count"
                  title={
                    zh
                      ? `${conversation.queued_count} 条待回复，其中 ${conversation.undelivered_count} 条尚未投递`
                      : `${conversation.queued_count} awaiting reply; ${conversation.undelivered_count} not yet delivered`
                  }
                >
                  {conversation.queued_count}
                </span>
              )}
            </button>
          ))}
          {!conversations.length && <div className="empty">{zh ? "还没有对话。" : "No conversations yet."}</div>}
        </div>
      </section>

      <section className="panel chat-panel">
        {error && <div className="error-banner chat-error">{error}</div>}
        {!selected ? (
          <div className="chat-empty">
            <strong>{zh ? "选择一个对话" : "Select a conversation"}</strong>
            <p>{zh ? "历史记录只有在你点开会话后才会加载进当前页面缓存。" : "History is fetched into this page cache only after you select a conversation."}</p>
          </div>
        ) : (
          <>
            <header className="chat-header">
              <div className="conversation-avatar large">{selected.agent_name.slice(0, 2).toUpperCase()}</div>
              <div>
                <h2>{selected.title}</h2>
                <span>{selected.agent_name} · {agents.find((agent) => agent.id === selected.agent_instance_id)?.status || selected.status}</span>
              </div>
              {selected.queued_count > 0 && (
                <div className="delivery-note">
                  {selected.undelivered_count > 0
                    ? zh
                      ? `${selected.undelivered_count} 条消息仍在可靠投递队列；Morrows 会在 Agent runtime 可用时送达。`
                      : `${selected.undelivered_count} message(s) remain in the durable delivery queue and will be delivered when the agent runtime is available.`
                    : zh
                      ? "消息已投递到 Agent runtime，等待回复。"
                      : "Messages have reached the agent runtime and are awaiting a reply."}
                </div>
              )}
            </header>

            <details className="conversation-summary-card">
              <summary>
                {zh ? "结构化摘要" : "Structured summary"}
                {summary?.current_state ? ` · ${summary.current_state}` : ""}
              </summary>
              {summary ? (
                <div className="conversation-summary-body">
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
                <p className="summary-empty">{zh ? "这个对话还没有结构化摘要。" : "This conversation has no structured summary yet."}</p>
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
                    <p>{message.body}</p>
                    <small>
                      {message.author_type === "human" ? (zh ? "你" : "You") : selected.agent_name}
                      {" · "}
                      {new Date(message.created_at).toLocaleTimeString(locale, { hour: "2-digit", minute: "2-digit" })}
                      {message.author_type === "human" && message.status === "queued"
                        ? message.delivery_status === "delivered"
                          ? (zh ? " · 已投递，等待回复" : " · delivered, awaiting reply")
                          : message.delivery_status === "claimed"
                            ? (zh ? " · 正在投递" : " · delivering")
                            : (zh ? " · 等待投递" : " · queued for delivery")
                        : ""}
                    </small>
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
                  if (event.key === "Enter" && !event.shiftKey) {
                    event.preventDefault();
                    event.currentTarget.form?.requestSubmit();
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
