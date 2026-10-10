#!/usr/bin/env python3
"""HTTP end-to-end check against a throwaway Morrows staging database.

Run with MORROWS_E2E_BASE=http://127.0.0.1:18986 python3 web/tests/task_revision_http_e2e.py.
Never point this destructive fixture at a production URL.
"""
import json
import os
from urllib.request import Request, urlopen
from urllib.error import HTTPError

base = os.environ.get("MORROWS_E2E_BASE", "http://127.0.0.1:18986").rstrip("/")
if not (base.startswith("http://127.0.0.1:") or base.startswith("http://localhost:")):
    raise RuntimeError("Fixture tests are restricted to localhost, never production")

def call(method, path, data=None, status=200):
    raw = None if data is None else json.dumps(data).encode()
    request = Request(base + path, data=raw, method=method,
                      headers={"Content-Type": "application/json"})
    try:
        with urlopen(request, timeout=20) as result:
            code = result.status
            body = json.load(result)
    except HTTPError as error:
        code = error.code
        body = json.loads(error.read().decode())
    assert code == status, (method, path, code, status, body)
    return body

def criterion(id):
    return {"id":id,"requirement":f"Independent proof for {id}","verification":"human_review"}

def draft(version, id):
    return {"expected_version":version,"reason":"Live evidence requires a revised check",
            "title":"Updated acceptance fixture","description":"New evidence-driven scope",
            "goal":"Finish audit correctly","scope":"gap-oriented validation",
            "acceptance_criteria":[criterion(id)]}

ready = call("POST","/api/tasks",{"title":"Ready revision fixture",
     "description":"Old scope","owner_actor_id":"human:fixture",
     "acceptance_criteria":[criterion("old")]})
rid=ready["id"]
initial=call("GET",f"/api/tasks/{rid}/revisions")
assert initial["active_version"] == 1
preview=call("POST",f"/api/tasks/{rid}/revisions/preview",draft(1,"new"))
assert preview["diff"]["acceptance_criteria"]["before"][0]["id"]=="old"
assert call("GET",f"/api/tasks/{rid}")["acceptance_criteria"][0]["id"]=="old"
applied=call("POST",f"/api/tasks/{rid}/revisions",draft(1,"new"))
assert applied["active_version"]==2 and applied["acceptance_version"]==2
assert applied["revisions"][0]["status"]=="applied"
assert call("GET",f"/api/tasks/{rid}")["acceptance_criteria"][0]["id"]=="new"
call("POST",f"/api/tasks/{rid}/revisions",draft(1,"obsolete"),status=409)

worker=call("POST","/api/agents",{"name":"revision-http-e2e-worker","capabilities":[]})
running=call("POST","/api/tasks",{"title":"Running revision fixture",
     "description":"Old scope","owner_actor_id":"human:fixture",
     "acceptance_criteria":[criterion("old")]})
tid=running["id"]
assignment=call("POST",f"/api/tasks/{tid}/claim",{"agent_instance_id":worker["id"],
                 "role":"executor","lease_seconds":900})
run=call("POST","/api/runs",{"assignment_id":assignment["id"],
                  "agent_instance_id":worker["id"]})
pending=call("POST",f"/api/tasks/{tid}/revisions",draft(1,"new"))
assert pending["active_version"]==1 and pending["revisions"][0]["status"]=="pending_ack"
assert call("GET",f"/api/tasks/{tid}")["acceptance_criteria"][0]["id"]=="old"
call("POST",f"/api/runs/{run['id']}/complete",{"agent_instance_id":worker["id"],
                 "payload":{}},status=409)
conflict=call("POST",f"/api/tasks/{tid}/revisions",draft(1,"another"),status=409)
assert "pending" in conflict["error"]
thread=call("GET",f"/api/tasks/{tid}/collaboration")
messages=thread["messages"]
assert (messages["items"] if isinstance(messages,dict) else messages),thread
rejected=call("POST",f"/api/tasks/{tid}/revisions/{pending['revisions'][0]['id']}/reject",
               {"reason":"Must negotiate a new evidence plan"})
assert rejected["revisions"][0]["status"]=="rejected"
assert rejected["active_version"]==1
again=call("POST",f"/api/tasks/{tid}/revisions",draft(1,"newer"))
assert again["revisions"][0]["status"]=="pending_ack"
call("POST",f"/api/tasks/{tid}/cancel")
assert call("GET",f"/api/tasks/{tid}/revisions")["revisions"][0]["status"]=="rejected"
call("POST",f"/api/tasks/{tid}/revisions",draft(1,"bad"),status=409)

completed=call("POST","/api/tasks",{"title":"Done cannot be rewritten",
     "owner_actor_id":"human:fixture"})
completed_assignment=call("POST",f"/api/tasks/{completed['id']}/claim",
     {"agent_instance_id":worker["id"],"role":"executor","lease_seconds":900})
completed_run=call("POST","/api/runs",
     {"assignment_id":completed_assignment["id"],"agent_instance_id":worker["id"]})
done=call("POST",f"/api/runs/{completed_run['id']}/complete",
     {"agent_instance_id":worker["id"],"payload":{"status":"passed"}})
assert done["status"]=="completed",done
call("POST",f"/api/tasks/{completed['id']}/revisions",draft(1,"bad"),status=409)
print(json.dumps({"pass":True,"ready_task":rid,"running_task":tid,
    "running_run":run["id"],"terminal_task":completed["id"],
    "covered":["REST preview/CAS","ready apply+history","pending blocks completion",
        "durable Task collaboration message","reject and retry","cancel race",
        "done task immutable"]},ensure_ascii=False))
