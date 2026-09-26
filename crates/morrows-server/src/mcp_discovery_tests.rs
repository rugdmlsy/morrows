use super::*;
use morrows_core::{CreateMemoryEntry, CreateProject};

fn caller(id: Option<Id>) -> Extension<Parts> {
    let mut request = axum::http::Request::builder();
    if let Some(id) = id {
        request = request.header("x-agent-instance-id", id.to_string());
    }
    Extension(request.body(()).unwrap().into_parts().0)
}

fn request<T: serde::de::DeserializeOwned>(value: Value) -> Parameters<T> {
    Parameters(serde_json::from_value(value).unwrap())
}

fn value(result: Result<String, String>) -> Value {
    serde_json::from_str(&result.unwrap()).unwrap()
}

#[tokio::test]
async fn memory_revise_preserves_omitted_fields_and_rejects_a_stale_read() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store.register_agent("context-writer", &[]).await.unwrap();
    let task = task(&store, "Audit", None, TaskState::Ready).await;
    store
        .claim_task(task.id, agent.id, "executor", 300)
        .await
        .unwrap();
    let original=store.create_context_revision(task.id,serde_json::from_value(json!({
        "goal":"Goal", "background":"Background", "constraints":{"freeze_requires":["Pass gate"]}
    })).unwrap()).await.unwrap();
    let mcp = MorrowsMcp::new(store.clone());
    let revised = value(
        mcp.memory_revise(
            request(json!({
                "task_id":task.id,"current_summary":"Progress only", "constraints":null,
                "expected_context_revision_id":original.id
            })),
            caller(Some(agent.id)),
        )
        .await,
    );
    assert_eq!(revised["goal"], original.goal);
    assert_eq!(revised["background"], original.background);
    assert_eq!(revised["constraints"], original.constraints);
    assert_eq!(revised["parent_revision_id"], json!(original.id));
    assert_eq!(
        revised["created_by_actor_id"],
        format!("agent:{}", agent.id)
    );
    assert!(mcp.memory_revise(request(json!({
        "task_id":task.id,"current_summary":"Stale", "expected_context_revision_id":original.id
    })),caller(Some(agent.id))).await.unwrap_err().contains("context changed"));
    assert!(
        mcp.memory_revise(request(json!({"task_id":task.id})), caller(Some(agent.id)))
            .await
            .is_err()
    );
    assert_eq!(
        store
            .get_current_context(task.id)
            .await
            .unwrap()
            .current_summary,
        "Progress only"
    );
}

#[tokio::test]
async fn live_and_persisted_context_use_the_newest_handoff() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store.register_agent("handoff-owner", &[]).await.unwrap();
    let t = task(&store, "Continuation", None, TaskState::Ready).await;
    store
        .create_context_revision(
            t.id,
            serde_json::from_value(json!({"goal":"Continue"})).unwrap(),
        )
        .await
        .unwrap();
    for next in ["Old action", "Current action"] {
        let assignment = store
            .claim_task(t.id, agent.id, "executor", 300)
            .await
            .unwrap();
        let run = store
            .start_run(assignment.id, agent.id, None)
            .await
            .unwrap();
        let milestone = store
            .create_run_milestone(
                run.id,
                agent.id,
                serde_json::from_value(json!({
                    "kind":"handoff_preparation",
                    "summary":next,
                    "completed":[],
                    "verified":[],
                    "remaining":[next],
                    "blockers":[next],
                    "next_step":next,
                    "next_plan":[next],
                    "execution_locations":[],
                    "artifact_ids":[],
                    "decision_ids":[]
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store
            .create_handoff(
                run.id,
                agent.id,
                serde_json::from_value(json!({
                    "milestone_id":milestone.id,
                    "summary":next, "completed":[], "remaining":[next], "blockers":[next]
                }))
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let mcp = MorrowsMcp::new(store);
    let live = value(
        mcp.task_context(request(json!({"task_id":t.id})), caller(Some(agent.id)))
            .await,
    );
    assert_eq!(
        live["collaboration"]["handoffs"]["items"][0]["remaining"][0],
        "Current action"
    );
    let persisted = value(
        mcp.context_package_assemble(request(json!({"task_id":t.id})), caller(Some(agent.id)))
            .await,
    );
    assert_eq!(persisted["next_action"], "Current action");
    assert_eq!(persisted["blockers"][0], "Current action");
}

async fn task(
    store: &Store,
    title: &str,
    project_id: Option<Id>,
    state: TaskState,
) -> morrows_core::Task {
    store
        .create_task(CreateTask {
            title: title.into(),
            project_id,
            description: "Task background".into(),
            owner_actor_id: "human:test".into(),
            state,
            priority: 0,
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn discovery_defaults_to_my_unfinished_work_and_can_browse_other_projects_and_agents() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = store.register_agent("reader-a", &[]).await.unwrap();
    let b = store.register_agent("reader-b", &[]).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Project".into(),
            description: "Project background".into(),
        })
        .await
        .unwrap();
    let mine = task(&store, "Mine", None, TaskState::Ready).await;
    let theirs = task(&store, "Theirs", Some(project.id), TaskState::Ready).await;
    let unassigned = task(&store, "Unassigned", Some(project.id), TaskState::Ready).await;
    let done = task(&store, "Done", Some(project.id), TaskState::Done).await;
    store
        .claim_task(mine.id, a.id, "executor", 300)
        .await
        .unwrap();
    store
        .claim_task(theirs.id, b.id, "executor", 300)
        .await
        .unwrap();
    let mcp = MorrowsMcp::new(store);
    let identity = value(mcp.whoami(caller(Some(a.id))).await);
    assert_eq!(identity["agent_instance_id"], json!(a.id));
    let own = value(mcp.task_list(request(json!({})), caller(Some(a.id))).await);
    assert_eq!(own["items"].as_array().unwrap().len(), 1);
    assert_eq!(own["items"][0]["id"], json!(mine.id));
    let other = value(
        mcp.task_list(
            request(json!({"agent_instance_id": b.id})),
            caller(Some(a.id)),
        )
        .await,
    );
    assert_eq!(other["items"][0]["id"], json!(theirs.id));
    let project_tasks = value(
        mcp.task_list(
            request(json!({"scope":"all", "project_id":project.id})),
            caller(Some(a.id)),
        )
        .await,
    );
    let ids: Vec<_> = project_tasks["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["id"].clone())
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&json!(theirs.id)) && ids.contains(&json!(unassigned.id)));
    let completed = value(
        mcp.task_list(
            request(json!({"scope":"all", "state":"done"})),
            caller(Some(a.id)),
        )
        .await,
    );
    assert_eq!(completed["items"][0]["id"], json!(done.id));
    let empty = value(
        mcp.task_list(
            request(json!({"project_id":project.id})),
            caller(Some(a.id)),
        )
        .await,
    );
    assert_eq!(empty["caller_agent_instance_id"], json!(a.id));
    assert!(empty["empty_reason"].is_string());
    assert!(
        mcp.task_list(
            request(json!({"scope":"all","agent_instance_id":b.id})),
            caller(Some(a.id))
        )
        .await
        .is_err()
    );
    let projects = value(
        mcp.project_list(request(json!({})), caller(Some(a.id)))
            .await,
    );
    assert_eq!(projects["items"][0]["id"], json!(project.id));
    let details = value(
        mcp.project_get(
            request(json!({"project_id":project.id})),
            caller(Some(a.id)),
        )
        .await,
    );
    assert_eq!(details["project"]["description"], "Project background");
}

#[tokio::test]
async fn unrelated_agent_can_read_every_task_view_but_cannot_mutate_it() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = store.register_agent("owner", &[]).await.unwrap();
    let b = store.register_agent("reader", &[]).await.unwrap();
    let t = task(&store, "Shared knowledge", None, TaskState::Ready).await;
    let assignment = store.claim_task(t.id, a.id, "executor", 300).await.unwrap();
    let run = store.start_run(assignment.id, a.id, None).await.unwrap();
    let milestone = store
        .create_run_milestone(
            run.id,
            a.id,
            serde_json::from_value(json!({
                "kind":"milestone",
                "summary":"owner reached a durable task boundary",
                "completed":["phase one"],
                "verified":["task state is durable"],
                "remaining":["phase two"],
                "blockers":[],
                "next_step":"start phase two",
                "next_plan":["start phase two","verify phase two"],
                "execution_locations":["/workspace/shared"],
                "artifact_ids":[],
                "decision_ids":[]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let mcp = MorrowsMcp::new(store.clone());
    let args = json!({"task_id":t.id});
    let reader_task = value(
        mcp.task_get(request(args.clone()), caller(Some(b.id)))
            .await,
    );
    assert_eq!(reader_task["id"], json!(t.id));
    assert_eq!(
        reader_task["execution"]["milestones"]["items"][0]["id"],
        json!(milestone.id)
    );
    assert!(
        reader_task["execution"]["my_runs"]["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    let reader_context = value(
        mcp.task_context(request(args.clone()), caller(Some(b.id)))
            .await,
    );
    assert_eq!(
        reader_context["execution"]["milestones"]["items"][0]["id"],
        json!(milestone.id)
    );
    assert!(
        mcp.memory_get(request(args.clone()), caller(Some(b.id)))
            .await
            .is_ok()
    );
    assert!(
        mcp.instructions_get(request(args.clone()), caller(Some(b.id)))
            .await
            .is_ok()
    );
    assert!(
        mcp.task_collaboration(request(args.clone()), caller(Some(b.id)))
            .await
            .is_ok()
    );
    assert!(
        mcp.task_events(request(args.clone()), caller(Some(b.id)))
            .await
            .is_ok()
    );
    assert_eq!(
        value(
            mcp.context_package_get(request(args.clone()), caller(Some(b.id)))
                .await
        ),
        Value::Null
    );
    assert!(
        store
            .get_latest_context_package(t.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        mcp.memory_revise(
            request(json!({"task_id":t.id,"goal":"overwrite"})),
            caller(Some(b.id))
        )
        .await
        .is_err()
    );
    assert!(
        mcp.context_package_assemble(request(args.clone()), caller(Some(b.id)))
            .await
            .is_err()
    );
    assert!(
        mcp.artifact_create(
            request(json!({"task_id":t.id,"input":{"title":"x","uri":"local:x"}})),
            caller(Some(b.id))
        )
        .await
        .is_err()
    );
    for id in [None, Some(Uuid::new_v4())] {
        assert!(mcp.whoami(caller(id)).await.is_err());
        assert!(
            mcp.task_list(request(json!({"scope":"all"})), caller(id))
                .await
                .is_err()
        );
        assert!(
            mcp.task_get(request(args.clone()), caller(id))
                .await
                .is_err()
        );
        assert!(
            mcp.task_context(request(args.clone()), caller(id))
                .await
                .is_err()
        );
        assert!(
            mcp.project_list(request(json!({})), caller(id))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn task_list_pages_are_bounded_and_do_not_duplicate_full_descriptions() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store.register_agent("reader", &[]).await.unwrap();
    for i in 0..5 {
        store
            .create_task(
                serde_json::from_value(
                    json!({"title": format!("Task {i}"), "description":"背景".repeat(200)}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
    }
    let mcp = MorrowsMcp::new(store);
    let mut seen = std::collections::HashSet::new();
    let mut offset = 0;
    loop {
        let page = value(
            mcp.task_list(
                request(json!({"scope":"all","limit":2,"offset":offset})),
                caller(Some(agent.id)),
            )
            .await,
        );
        for item in page["items"].as_array().unwrap() {
            assert!(seen.insert(item["id"].as_str().unwrap().to_string()));
            assert_eq!(
                item["description_preview"]
                    .as_str()
                    .unwrap()
                    .chars()
                    .count(),
                240
            );
            assert_eq!(item["description_truncated"], true);
            assert!(item.get("description").is_none());
        }
        match page["next_offset"].as_i64() {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(seen.len(), 5);
    let exhausted = value(
        mcp.task_list(
            request(json!({"scope":"all", "offset":5})),
            caller(Some(agent.id)),
        )
        .await,
    );
    assert!(
        exhausted["empty_reason"]
            .as_str()
            .unwrap()
            .contains("offset=0")
    );
    for args in [
        json!({"limit":0}),
        json!({"limit":101}),
        json!({"offset":-1}),
        json!({"state":"bogus"}),
        json!({"project_id":"bad"}),
    ] {
        assert!(
            mcp.task_list(request(args), caller(Some(agent.id)))
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn live_context_includes_background_and_current_visible_memory_without_history_dump() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = store.register_agent("author", &[]).await.unwrap();
    let b = store.register_agent("reader", &[]).await.unwrap();
    let project = store
        .create_project(CreateProject {
            name: "Project".into(),
            description: "Why this project exists".into(),
        })
        .await
        .unwrap();
    let t = task(&store, "Context", Some(project.id), TaskState::Ready).await;
    let revision = store.create_context_revision(t.id, serde_json::from_value(json!({
        "goal":"Deliver result", "background":"Why this task matters", "current_summary":"In review",
        "constraints":{"acceptance_criteria":["Regression tests pass"]}
    })).unwrap()).await.unwrap();
    let old = store.create_memory_entry(serde_json::from_value::<CreateMemoryEntry>(json!({
        "scope_type":"project", "project_id":project.id, "title":"Old rule", "content":"old", "source_kind":"manual"
    })).unwrap()).await.unwrap();
    let current = store.create_memory_entry(serde_json::from_value::<CreateMemoryEntry>(json!({
        "scope_type":"project", "project_id":project.id, "title":"Current rule", "content":"current", "source_kind":"manual", "supersedes_memory_id":old.id
    })).unwrap()).await.unwrap();
    for (id, content) in [(a.id, "author secret"), (b.id, "reader preference")] {
        store.create_memory_entry(serde_json::from_value::<CreateMemoryEntry>(json!({
            "scope_type":"agent", "agent_instance_id":id, "title":"Private", "content":content, "source_kind":"manual", "visibility":"private"
        })).unwrap()).await.unwrap();
    }
    for i in 0..7 {
        store
            .create_decision(
                t.id,
                a.id,
                CreateDecision {
                    title: format!("Decision {i}"),
                    rationale: "Evidence".into(),
                },
            )
            .await
            .unwrap();
    }
    let mcp = MorrowsMcp::new(store.clone());
    let events_before = store.task_events(t.id).await.unwrap().len();
    let context = value(
        mcp.task_context(request(json!({"task_id":t.id})), caller(Some(b.id)))
            .await,
    );
    assert_eq!(context["project"]["description"], "Why this project exists");
    assert_eq!(context["context"]["id"], json!(revision.id));
    assert_eq!(
        context["context"]["constraints"]["acceptance_criteria"][0],
        "Regression tests pass"
    );
    assert!(context["missing_context"].as_array().unwrap().is_empty());
    let memory = context["memory"]["items"].as_array().unwrap();
    assert_eq!(memory.len(), 2);
    assert!(memory.iter().any(|m| m["id"] == json!(current.id)));
    assert!(!context.to_string().contains("author secret"));
    assert_eq!(
        context["collaboration"]["decisions"]["items"]
            .as_array()
            .unwrap()
            .len(),
        5
    );
    assert_eq!(context["collaboration"]["decisions"]["next_offset"], 5);
    assert!(context["collaboration"].get("messages").is_none());
    assert_eq!(store.task_events(t.id).await.unwrap().len(), events_before);
    assert!(
        store
            .get_latest_context_package(t.id)
            .await
            .unwrap()
            .is_none()
    );
    let more = value(
        mcp.task_collaboration(
            request(json!({"task_id":t.id,"section":"decisions","limit":5,"offset":5})),
            caller(Some(b.id)),
        )
        .await,
    );
    assert_eq!(more["decisions"]["items"].as_array().unwrap().len(), 2);
    assert!(more.get("messages").is_none());
    let memory_only = value(
        mcp.memory_get(request(json!({"task_id":t.id})), caller(Some(b.id)))
            .await,
    );
    assert!(memory_only.get("collaboration").is_none());
    assert!(memory_only.get("task").is_none());
    let project_view = value(
        mcp.project_get(
            request(json!({"project_id":project.id})),
            caller(Some(b.id)),
        )
        .await,
    );
    assert_eq!(project_view["memory"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(project_view["memory"]["items"][0]["id"], json!(current.id));

    // A compact current view must not remove the employee's route to older
    // evidence. History is paginated and still enforces private-memory isolation.
    let newer_revision = store
        .create_context_revision(
            t.id,
            serde_json::from_value(json!({
                "goal":"Revised goal", "background":"Additional evidence"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let mut seen_memories = std::collections::HashSet::new();
    let mut seen_revisions = std::collections::HashSet::new();
    let mut offset = 0;
    loop {
        let history = value(
            mcp.memory_get(
                request(json!({
                    "task_id":t.id, "context_revision_id":revision.id,
                    "include_superseded":true, "include_context_history":true,
                    "limit":1, "offset":offset
                })),
                caller(Some(b.id)),
            )
            .await,
        );
        assert_eq!(history["context"]["id"], json!(revision.id));
        assert_eq!(history["context"]["background"], "Why this task matters");
        assert_eq!(
            history["current_context_revision_id"],
            json!(newer_revision.id)
        );
        assert_eq!(history["context_is_current"], false);
        assert!(!history.to_string().contains("author secret"));
        for memory in history["long_term_memory"].as_array().unwrap() {
            assert!(seen_memories.insert(memory["id"].as_str().unwrap().to_string()));
            if memory["id"] == json!(old.id) {
                assert_eq!(memory["content"], "old");
                assert_eq!(memory["source_kind"], "manual");
            }
        }
        for context in history["context_history"]["items"].as_array().unwrap() {
            assert!(seen_revisions.insert(context["id"].as_str().unwrap().to_string()));
            assert!(context["background"].is_string());
        }
        let next = [
            history["next_offset"].as_i64(),
            history["context_history"]["next_offset"].as_i64(),
        ]
        .into_iter()
        .flatten()
        .max();
        match next {
            Some(next) => offset = next,
            None => break,
        }
    }
    assert_eq!(seen_memories.len(), 3);
    assert!(seen_memories.contains(&old.id.to_string()));
    assert_eq!(seen_revisions.len(), 2);
    let other = task(&store, "Different task", None, TaskState::Ready).await;
    assert!(
        mcp.memory_get(
            request(json!({
                "task_id":other.id, "context_revision_id":revision.id
            })),
            caller(Some(b.id))
        )
        .await
        .unwrap_err()
        .contains("does not belong")
    );
    let project_history = value(
        mcp.project_get(
            request(json!({
                "project_id":project.id, "include_superseded":true
            })),
            caller(Some(b.id)),
        )
        .await,
    );
    assert_eq!(
        project_history["memory"]["items"].as_array().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn project_previews_are_explicit_and_full_text_remains_readable() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store.register_agent("project-reader", &[]).await.unwrap();
    let description = "背景内容".repeat(100);
    let project = store
        .create_project(CreateProject {
            name: "Long background".into(),
            description: description.clone(),
        })
        .await
        .unwrap();
    let mcp = MorrowsMcp::new(store);
    let page = value(
        mcp.project_list(request(json!({})), caller(Some(agent.id)))
            .await,
    );
    let preview = &page["items"][0];
    assert_eq!(
        preview["description_preview"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        240
    );
    assert_eq!(preview["description_truncated"], true);
    assert!(preview.get("description").is_none());
    let full = value(
        mcp.project_get(
            request(json!({"project_id":project.id})),
            caller(Some(agent.id)),
        )
        .await,
    );
    assert_eq!(full["project"]["description"], description);
}

#[tokio::test]
async fn imported_freeze_gates_are_recognized_without_inventing_verified_results() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store.register_agent("gate-reader", &[]).await.unwrap();
    let t = task(&store, "Freeze audit", None, TaskState::Ready).await;
    let gates = json!([
        "audit_complete",
        "all regressions explained",
        "canary gates pass"
    ]);
    store
        .create_context_revision(
            t.id,
            serde_json::from_value(json!({
                "constraints":{"freeze_requires":gates, "acceptance_criteria":{}}
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let mcp = MorrowsMcp::new(store);
    let context = value(
        mcp.task_context(request(json!({"task_id":t.id})), caller(Some(agent.id)))
            .await,
    );
    assert_eq!(
        context["acceptance_criteria_paths"],
        json!(["context.constraints.freeze_requires"])
    );
    assert!(
        !context["missing_context"]
            .as_array()
            .unwrap()
            .contains(&json!("structured_acceptance_criteria"))
    );
    assert_eq!(context["context"]["constraints"]["freeze_requires"], gates);
    assert!(context.get("verified_results").is_none());
}

#[tokio::test]
async fn incomplete_context_is_explicit_and_events_can_be_paged_and_filtered() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store.register_agent("reader", &[]).await.unwrap();
    let t = task(&store, "Incomplete", None, TaskState::Ready).await;
    store
        .create_decision(
            t.id,
            agent.id,
            CreateDecision {
                title: "decision".into(),
                rationale: "reason".into(),
            },
        )
        .await
        .unwrap();
    let mcp = MorrowsMcp::new(store);
    let context = value(
        mcp.task_context(request(json!({"task_id":t.id})), caller(Some(agent.id)))
            .await,
    );
    let missing = context["missing_context"].as_array().unwrap();
    assert!(missing.contains(&json!("context_background")));
    assert!(missing.contains(&json!("structured_acceptance_criteria")));
    let page = value(
        mcp.task_events(
            request(json!({"task_id":t.id,"limit":1})),
            caller(Some(agent.id)),
        )
        .await,
    );
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["next_offset"], 1);
    let filtered = value(
        mcp.task_events(
            request(json!({"task_id":t.id,"event_type":"task.created"})),
            caller(Some(agent.id)),
        )
        .await,
    );
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["items"][0]["event_type"], "task.created");
}
