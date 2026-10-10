import { useState } from "react";
import { api } from "./api";

export type Criterion = {
  id: string; requirement: string; verification: string;
  allow_not_applicable: boolean; requires_independent_review: boolean;
  required_artifact_kinds: string[];
  check?: { command: string; machine: string; cwd: string; read_only: boolean; timeout_s: number };
  artifact_check?: { path: string; machine: string };
};
type Receipt = { id: string; criterion_id: string; verdict: string; kind: string; created_at: string; detail: unknown };
type Status = {criterion_id:string;verdict:string;missing_evidence:string[]};
type Record = { acceptance_version: number; completed_at: string; criterion_verdicts: { criterion_id: string; verdict: string }[]; evidence: unknown; verification_receipts: Receipt[] };

export function AcceptanceEditor({ value, onChange, zh }: {value: Criterion[]; onChange: (v:Criterion[])=>void; zh:boolean}) {
  const update = (index:number, patch:Partial<Criterion>) => onChange(value.map((c,i)=>i===index?{...c,...patch}:c));
  return <fieldset className="acceptance-editor"><legend>{zh?"验收条件（可选）":"Acceptance criteria (optional)"}</legend>
    <p>{zh?"文字说明不会自动建立验收门禁。发布后的条件固定；每条条件可单独配置验证方式。":"Description text does not create a completion gate. Published criteria are fixed; configure verification per criterion."}</p>
    {value.map((c,i)=><div className="criterion-editor" key={i}>
      <input aria-label={zh?"稳定 ID":"Stable ID"} value={c.id} onChange={e=>update(i,{id:e.target.value})}/>
      <input aria-label={zh?"验收要求":"Requirement"} placeholder={zh?"验收要求":"Requirement"} value={c.requirement} onChange={e=>update(i,{requirement:e.target.value})} required/>
      <select aria-label={zh?"验证方式":"Verification mode"} value={c.verification} onChange={e=>update(i,{verification:e.target.value,
        check:e.target.value==="deterministic_check"?{command:"",cwd:"",machine:"",read_only:true,timeout_s:30}:undefined,
        artifact_check:e.target.value==="artifact_check"?{path:"",machine:""}:undefined})}>
        <option value="self_attested">{zh?"执行者证据":"Executor evidence"}</option>
        <option value="deterministic_check">{zh?"实际命令检查":"Runtime command check"}</option>
        <option value="artifact_check">{zh?"文件存在与可读性":"File existence/readability"}</option>
        <option value="independent_review">{zh?"独立审查":"Independent review"}</option>
        <option value="human_review">{zh?"人工验收":"Human review"}</option>
      </select>
      {c.check && <>
        <input aria-label="Command" placeholder={zh?"只读检查命令":"Read-only check command"} value={c.check.command} onChange={e=>update(i,{check:{...c.check!,command:e.target.value}})} required/>
        <input aria-label="Machine" placeholder="Machine" value={c.check.machine} onChange={e=>update(i,{check:{...c.check!,machine:e.target.value}})} required/>
        <input aria-label="Working directory" placeholder="/absolute/workspace" value={c.check.cwd} onChange={e=>update(i,{check:{...c.check!,cwd:e.target.value}})} required/>
      </>}
      {c.artifact_check && <>
        <input aria-label="Artifact path" placeholder="/absolute/file" value={c.artifact_check.path} onChange={e=>update(i,{artifact_check:{...c.artifact_check!,path:e.target.value}})} required/>
        <input aria-label="Artifact machine" placeholder="Machine" value={c.artifact_check.machine} onChange={e=>update(i,{artifact_check:{...c.artifact_check!,machine:e.target.value}})} required/>
      </>}
      <label><input type="checkbox" checked={c.requires_independent_review} onChange={e=>update(i,{requires_independent_review:e.target.checked})}/>{zh?"另需独立审查":"Also require independent review"}</label>
      <label><input type="checkbox" checked={c.allow_not_applicable} onChange={e=>update(i,{allow_not_applicable:e.target.checked})}/>{zh?"允许说明不适用":"Allow justified not applicable"}</label>
      <button type="button" onClick={()=>onChange(value.filter((_,j)=>j!==i))}>{zh?"删除":"Remove"}</button>
    </div>)}
    <button type="button" onClick={()=>onChange([...value,{id:crypto.randomUUID(),requirement:"",verification:"self_attested",allow_not_applicable:false,requires_independent_review:false,required_artifact_kinds:[]}])}>{zh?"添加条件":"Add criterion"}</button>
    {value.length>0 && <button type="button" onClick={()=>onChange(value.map(c=>({...c,requires_independent_review:true})))}>{zh?"所有条件需独立审查":"Require independent review for all"}</button>}
  </fieldset>;
}

export function AcceptanceDetail({ taskId, criteria, data, zh, executorRunId, contextRevisionId, acceptanceVersion, refresh }: {taskId:string;criteria:Criterion[];data:{receipts:Receipt[];completion_records:Record[];criterion_statuses?:Status[]}|null;zh:boolean;executorRunId?:string;contextRevisionId?:string|null;acceptanceVersion:number;refresh:()=>void}) {
  const [error,setError]=useState("");
  async function humanVerdict(id:string,verdict:string) {
    const rationale=window.prompt(zh?"填写验收理由 / 修改要求":"Enter rationale / requested correction");
    if(!rationale || !executorRunId)return;
    try {
      await api("/api/tasks/"+taskId+"/human-review",{method:"POST",body:JSON.stringify({criterion_id:id,executor_run_id:executorRunId,verdict,rationale,context_revision_id:contextRevisionId??null,acceptance_version:acceptanceVersion})});
      refresh();
    } catch(e) {setError(String(e));}
  }
  if(!criteria.length && !data?.completion_records.length)return null;
  return <section className="acceptance-detail"><h3>{zh?"验收合同与完成依据":"Acceptance contract and completion evidence"}</h3>
    {error && <p role="alert">{error}</p>}
    <table><thead><tr><th>ID</th><th>{zh?"要求":"Requirement"}</th><th>{zh?"验证":"Verification"}</th><th>{zh?"状态":"Status"}</th></tr></thead>
      <tbody>{criteria.map(c=>{
        const current=data?.criterion_statuses?.find(s=>s.criterion_id===c.id);
        const completed=data?.completion_records[0]?.criterion_verdicts.find(v=>v.criterion_id===c.id);
        return <tr key={c.id}><td>{c.id}</td><td>{c.requirement}</td><td>{c.verification}{c.requires_independent_review?" + review":""}</td><td>{completed?.verdict || current?.verdict || "BLOCKED"}
          {c.verification==="human_review" && executorRunId && <><button onClick={()=>void humanVerdict(c.id,"PASS")}>PASS</button><button onClick={()=>void humanVerdict(c.id,"FAIL")}>FAIL</button></>}
        </td></tr>;
      })}</tbody></table>
    {data?.completion_records.map((r,i)=><details key={i}><summary>{zh?"完成记录":"Completion record"} · {r.completed_at} · v{r.acceptance_version}</summary><pre>{JSON.stringify(r,null,2)}</pre></details>)}
    {data?.receipts.map(r=><details key={r.id}><summary>{r.criterion_id} · {r.kind} · {r.verdict} · {r.created_at}</summary><pre>{JSON.stringify(r.detail,null,2)}</pre></details>)}
  </section>;
}

