import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import type { Locale } from "./i18n";

type Request = {
  id: string;
  task_id: string;
  agent_instance_id: string;
  role: string;
  reason: string;
  status: string;
  resolution: string | null;
  assignment_id: string | null;
};
type Page = { items: Request[]; next_offset: number | null };

export default function AssignmentRequests({ locale, tasks, agents, onSelectTask, onResolved }: {
  locale: Locale;
  tasks: { id: string; title: string }[];
  agents: { id: string; name: string; display_name: string }[];
  onSelectTask: (id: string) => void;
  onResolved: () => Promise<void>;
}) {
  const zh = locale === "zh-CN";
  const [page, setPage] = useState<Page>({ items: [], next_offset: null });
  const [offset, setOffset] = useState(0);
  const [history, setHistory] = useState(false);
  const [notes, setNotes] = useState<Record<string, string>>({});
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const refresh = useCallback(async () => {
    const result = await api<Page>(`/api/assignment-requests?include_resolved=${history}&limit=20&offset=${offset}`);
    setPage(result);
  }, [history, offset]);

  useEffect(() => {
    let active = true;
    async function poll() {
      try {
        const result = await api<Page>(`/api/assignment-requests?include_resolved=${history}&limit=20&offset=${offset}`);
        if (active) { setPage(result); setLoadError(null); }
      } catch (e) { if (active) setLoadError(String(e)); }
    }
    void poll();
    const timer = window.setInterval(() => void poll(), 5000);
    return () => { active = false; window.clearInterval(timer); };
  }, [history, offset]);

  async function resolve(request: Request, action: "approve" | "reject") {
    setBusy(request.id);
    setError(null);
    try {
      await api(`/api/assignment-requests/${request.id}/resolve`, {
        method: "POST", body: JSON.stringify({ action, resolution: notes[request.id], lease_seconds: 900 }),
      });
      await refresh();
      await onResolved();
    } catch (e) { setError(String(e)); }
    finally { setBusy(null); }
  }

  return <details className="assignment-requests">
    <summary>{zh ? "接手申请" : "Assignment requests"} · {page.items.length}{page.next_offset !== null ? "+" : ""}</summary>
    <p>{zh ? "批准后分配现有任务，随后可在任务详情中启动执行。" : "Approval assigns the existing task. Start execution from its task details afterward."}</p>
    <label><input type="checkbox" checked={history} onChange={e => { setHistory(e.target.checked); setOffset(0); }} /> {zh ? "包含已处理记录" : "Include resolved history"}</label>
    {error && <div className="error-banner">{error}</div>}
    {loadError && <div className="error-banner">{loadError}</div>}
    {!page.items.length && <p>{zh ? "本页没有申请。" : "No requests on this page."}</p>}
    {page.items.map(request => <article key={request.id} className="assignment-request">
      <button type="button" className="secondary" onClick={() => onSelectTask(request.task_id)}>{tasks.find(task => task.id === request.task_id)?.title ?? request.task_id}</button>
      <strong>{agents.find(agent => agent.id === request.agent_instance_id)?.display_name || agents.find(agent => agent.id === request.agent_instance_id)?.name || request.agent_instance_id}</strong>
      <small>{request.agent_instance_id} · {request.role} · {request.status}</small>
      <p>{request.reason}</p>
      {request.resolution && <p>{request.resolution}</p>}
      {request.assignment_id && <small>{zh ? "分配" : "Assignment"}: {request.assignment_id}</small>}
      {request.status === "pending" && <>
        <textarea aria-label={zh ? "处理说明" : "Resolution reason"} placeholder={zh ? "处理说明（会保留在历史中）" : "Resolution reason (retained in history)"}
          value={notes[request.id] ?? ""} onChange={e => setNotes(current => ({ ...current, [request.id]: e.target.value }))} />
        <div className="task-browser-actions">
          <button type="button" disabled={busy !== null || !notes[request.id]?.trim()} onClick={() => void resolve(request, "approve")}>{zh ? "批准并分配" : "Approve and assign"}</button>
          <button type="button" className="secondary" disabled={busy !== null || !notes[request.id]?.trim()} onClick={() => void resolve(request, "reject")}>{zh ? "拒绝" : "Reject"}</button>
        </div>
      </>}
    </article>)}
    <div className="task-browser-actions">
      <button type="button" className="secondary" disabled={offset === 0} onClick={() => setOffset(Math.max(0, offset - 20))}>{zh ? "上一页" : "Previous"}</button>
      <button type="button" className="secondary" disabled={page.next_offset === null} onClick={() => setOffset(page.next_offset!)}>{zh ? "下一页" : "Next"}</button>
    </div>
  </details>;
}
