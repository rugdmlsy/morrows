import { useCallback, useEffect, useState } from "react";
import { api } from "./api";
import { AcceptanceEditor, type Criterion } from "./Acceptance";

type Spec = {
  title: string; description: string; goal: string; scope: string; acceptance_criteria: Criterion[];
};
type Revision = {
  id: string; version: number; base_version: number;
  status: "applied" | "pending_ack" | "rejected";
  author_actor_id: string; reason: string; before: Spec; after: Spec;
  diff: Record<string, { before: unknown; after: unknown }>;
  ack_impact?: string | null; updated_plan?: string[] | null; ack_agent_id?: string | null;
  created_at: string; resolved_at?: string | null;
};
type History = { active_version: number; acceptance_version: number; revisions: Revision[] };
const errorText = (error: unknown) => error instanceof Error ? error.message : String(error);

export function TaskRevisionPanel({ taskId, taskState, zh, onApplied }: { taskId: string; taskState: string; zh: boolean; onApplied: () => void }) {
  const [history, setHistory] = useState<History | null>(null);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<Spec | null>(null);
  const [reason, setReason] = useState("");
  const [preview, setPreview] = useState<Record<string, unknown> | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const reload = useCallback(async () => setHistory(await api<History>(`/api/tasks/${taskId}/revisions`)), [taskId]);
  useEffect(() => {
    const first = window.setTimeout(() => void reload().catch(e => setError(errorText(e))), 0);
    const timer = window.setInterval(() => void reload().catch(() => {}), 5000);
    return () => { window.clearTimeout(first); window.clearInterval(timer); };
  }, [reload]);
  const active = history?.revisions.find(r => r.status === "applied" && r.version === history.active_version);
  const pending = history?.revisions.find(r => r.status === "pending_ack");
  const mutate = (patch: Partial<Spec>) => setDraft(value => value ? { ...value, ...patch } : value);
  function begin() {
    const current = active?.after;
    if (!current) return;
    setDraft({ ...current, acceptance_criteria: current.acceptance_criteria.map(item => ({ ...item })) });
    setReason(""); setPreview(null); setEditing(true); setError("");
  }
  function payload() {
    if (!draft || !history || !active) throw new Error("Task revision unavailable");
    // Only include changed fields. Empty acceptance_criteria is a deliberate removal.
    const changes = Object.fromEntries((["title", "description", "goal", "scope", "acceptance_criteria"] as const)
      .filter(key => JSON.stringify(draft[key]) !== JSON.stringify(active.after[key]))
      .map(key => [key, draft[key]]));
    return { expected_version: history.active_version, reason, ...changes };
  }
  async function submit(previewOnly: boolean) {
    setBusy(true); setError("");
    try {
      const body = JSON.stringify(payload());
      if (previewOnly) {
        setPreview(await api<Record<string, unknown>>(`/api/tasks/${taskId}/revisions/preview`, { method: "POST", body }));
      } else {
        if (!preview) throw new Error(zh ? "请先预览变更" : "Preview the changes first");
        await api(`/api/tasks/${taskId}/revisions`, { method: "POST", body });
        setEditing(false); setPreview(null); await reload(); onApplied();
      }
    } catch (e) { setError(errorText(e)); } finally { setBusy(false); }
  }
  async function reject(id: string) {
    const why = window.prompt(zh ? "拒绝理由" : "Reason for rejection");
    if (!why) return;
    setBusy(true);setError("");
    try {
      await api(`/api/tasks/${taskId}/revisions/${id}/reject`,{method:"POST",body:JSON.stringify({reason:why})});
      await reload();onApplied();
    } catch(e) {setError(errorText(e));} finally{setBusy(false);}
  }
  const beforeAfter = (value: unknown) => typeof value === "string" ? value : JSON.stringify(value, null, 2);
  return <section className="task-revision-panel" aria-label={zh ? "任务修订" : "Task Revisions"}>
    <div className="task-revision-header">
      <div><strong>{zh ? "任务修订" : "Task revisions"}</strong><small>v{history?.active_version ?? "…"} · {zh ? "正式验收" : "Acceptance"} v{history?.acceptance_version ?? "…"}</small></div>
      {!editing && active && !pending && !["done", "cancelled"].includes(taskState) && <button type="button" className="secondary" onClick={begin}>{zh ? "修订任务" : "Revise task"}</button>}
    </div>
    {error && <p role="alert" className="revision-error">{error}</p>}
    {pending && <div className="revision-pending" role="status"><strong>{zh ? "等待执行者确认" : "Awaiting executor acknowledgment"} · v{pending.version}</strong>
      <p>{zh ? "旧版验收和新检查已暂停提交。执行者须阅读差异、提交影响分析与新计划，再通过 task_revision_ack 生效；运行中的外部操作不会被强制停止。" : "Completion and new checks are gated until the executor acknowledges with impact and updated plan. In-flight external operations are not forcibly stopped."}</p>
      <button type="button" className="secondary" disabled={busy} onClick={() => void reject(pending.id)}>{zh ? "拒绝提议" : "Reject proposal"}</button>
    </div>}
    {editing && draft && <form className="revision-editor" onSubmit={e => {e.preventDefault();void submit(false);}}>
      <label>{zh ? "修订原因" : "Revision reason"}<textarea required value={reason} onChange={e => {setReason(e.target.value);setPreview(null);}}/></label>
      <label>{zh ? "标题" : "Title"}<input value={draft.title} onChange={e=>{mutate({title:e.target.value});setPreview(null);}}/></label>
      <label>{zh ? "任务描述" : "Description"}<textarea value={draft.description} onChange={e=>{mutate({description:e.target.value});setPreview(null);}}/></label>
      <label>{zh ? "目标" : "Goal"}<textarea value={draft.goal} onChange={e=>{mutate({goal:e.target.value});setPreview(null);}}/></label>
      <label>{zh ? "范围" : "Scope"}<textarea value={draft.scope} onChange={e=>{mutate({scope:e.target.value});setPreview(null);}}/></label>
      <AcceptanceEditor value={draft.acceptance_criteria} onChange={v=>{mutate({acceptance_criteria:v});setPreview(null);}} zh={zh}/>
      <div className="revision-actions">
        <button type="button" className="secondary" disabled={busy || !reason.trim()} onClick={() => void submit(true)}>{zh ? "预览差异" : "Preview changes"}</button>
        <button type="submit" disabled={busy || !preview}>{zh ? "提交修订" : "Submit revision"}</button>
        <button type="button" className="secondary" onClick={()=>setEditing(false)}>{zh ? "取消" : "Cancel"}</button>
      </div>
      {preview && <details open><summary>{zh ? "变更预览（提交前）" : "Preview before submitting"}</summary>
        <div className="revision-diff">{Object.entries((preview.diff || {}) as Record<string, {before: unknown;after:unknown}>).map(([key,change])=><div key={key}>
          <strong>{key}</strong><div className="revision-diff-columns"><pre>{beforeAfter(change.before)}</pre><pre>{beforeAfter(change.after)}</pre></div>
        </div>)}</div>
      </details>}
    </form>}
    <div className="revision-history">{history?.revisions.map(rev=><details key={rev.id} open={rev.status==="pending_ack"}>
      <summary>v{rev.version} · {rev.status} · {rev.author_actor_id} · {new Date(rev.created_at).toLocaleString()}</summary>
      <p>{rev.reason}</p>
      <div className="revision-diff">{Object.entries(rev.diff).map(([key,value])=><div key={key}><strong>{key}</strong><div className="revision-diff-columns">
        <pre>{beforeAfter(value.before)}</pre><pre>{beforeAfter(value.after)}</pre>
      </div></div>)}</div>
      {rev.ack_impact && <p>{zh ? "执行者影响说明：" : "Executor impact: "}{rev.ack_impact}</p>}
      {rev.updated_plan && <pre>{JSON.stringify(rev.updated_plan,null,2)}</pre>}
    </details>)}</div>
  </section>;
}
