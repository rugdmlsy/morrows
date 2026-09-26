use morrows_core::*;
use morrows_store::Store;
use serde_json::{Value, json};

async fn fixture(store: &Store) -> (Id, Id, Id, Id) {
    let agent = store.register_agent("worker", &[]).await.unwrap();
    let project = store
        .create_project(serde_json::from_value(json!({"name":"Project"})).unwrap())
        .await
        .unwrap();
    let task = store
        .create_task(
            serde_json::from_value(
                json!({"title":"Research", "project_id":project.id,"state":"ready"}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let context = store.create_context_revision(task.id,serde_json::from_value(json!({"goal":"Verify", "background":"Source references", "current_summary":"Latest observations", "constraints":{"freeze_requires":["Original receipts verified","Canary passes"]}})).unwrap()).await.unwrap();
    (agent.id, project.id, task.id, context.id)
}

fn publication(task: Id, context: Id, key: &str) -> PublishProjectMemory {
    serde_json::from_value(json!({"task_id":task,"idempotency_key":key,"title":"Reusable observation","content":{"finding":"Unknown remains unknown","raw_reference":"original"},"verification_status":"unverified","basis":"Historical observation, not revalidated","context_revision_id":context})).unwrap()
}

#[tokio::test]
async fn native_document_id_is_stable_unique_and_preserves_legacy_retry_shape() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (agent, _, task, context) = fixture(&store).await;
    store
        .claim_task(task, agent, "executor", 300)
        .await
        .unwrap();
    let legacy = publication(task, context, "legacy");
    assert!(
        serde_json::to_value(&legacy)
            .unwrap()
            .get("new_memory_id")
            .is_none()
    );
    let mut input = publication(task, context, "native-file");
    let requested = Id::new_v4();
    input.new_memory_id = Some(requested);
    let entry = store
        .publish_project_memory(agent, input.clone())
        .await
        .unwrap();
    assert_eq!(entry.id, requested);
    assert_eq!(
        store
            .publish_project_memory(agent, input.clone())
            .await
            .unwrap()
            .id,
        requested
    );
    let mut collision = input.clone();
    collision.idempotency_key = "different-operation".into();
    assert!(
        store
            .publish_project_memory(agent, collision)
            .await
            .unwrap_err()
            .to_string()
            .contains("already in use")
    );
    input.idempotency_key = "invalid-revision".into();
    input.new_memory_id = Some(Id::new_v4());
    input.supersedes_memory_id = Some(requested);
    assert!(
        store
            .publish_project_memory(agent, input)
            .await
            .unwrap_err()
            .to_string()
            .contains("not a revision")
    );
    assert_eq!(
        store.get_memory_entry(requested).await.unwrap().content,
        entry.content
    );
}

#[tokio::test]
async fn project_move_cannot_redirect_a_prepared_native_publication() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (agent, project, task, context) = fixture(&store).await;
    store
        .claim_task(task, agent, "executor", 300)
        .await
        .unwrap();
    let other = store
        .create_project(serde_json::from_value(json!({"name":"Other"})).unwrap())
        .await
        .unwrap();
    let mut input = publication(task, context, "pin-project");
    assert!(
        serde_json::to_value(&input)
            .unwrap()
            .get("expected_project_id")
            .is_none()
    );
    input.expected_project_id = Some(project);
    input.new_memory_id = Some(Id::new_v4());
    store.set_task_project(task, Some(other.id)).await.unwrap();
    assert!(
        store
            .publish_project_memory(agent, input)
            .await
            .unwrap_err()
            .to_string()
            .contains("task project changed")
    );
    assert!(
        store
            .context_memories_page(None, Some(other.id), None, true, 20, 0)
            .await
            .unwrap()
            .items
            .is_empty()
    );
}

#[tokio::test]
async fn project_publication_binds_scope_retains_history_and_handles_retry_and_races() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (agent, project, task, context) = fixture(&store).await;
    let input = publication(task, context, "first");
    assert!(
        store
            .publish_project_memory(agent, input.clone())
            .await
            .is_err()
    );
    store
        .claim_task(task, agent, "executor", 300)
        .await
        .unwrap();
    let first = store
        .publish_project_memory(agent, input.clone())
        .await
        .unwrap();
    assert_eq!(first.project_id, Some(project));
    assert_eq!(first.content, input.content);
    assert_eq!(first.provenance.as_ref().unwrap()["task_id"], json!(task));
    assert_eq!(first.provenance.as_ref().unwrap()["server_verified"], false);
    assert_eq!(
        store
            .publish_project_memory(agent, input.clone())
            .await
            .unwrap()
            .id,
        first.id
    );
    let mut changed = input.clone();
    changed.content = json!("different");
    assert!(
        store
            .publish_project_memory(agent, changed)
            .await
            .unwrap_err()
            .to_string()
            .contains("different content")
    );
    let stranger = store.register_agent("stranger", &[]).await.unwrap();
    assert!(
        store
            .publish_project_memory(stranger.id, input)
            .await
            .is_err()
    );
    let mut a = publication(task, context, "revision-a");
    a.supersedes_memory_id = Some(first.id);
    let mut b = a.clone();
    b.idempotency_key = "revision-b".into();
    let (a, b) = tokio::join!(
        store.publish_project_memory(agent, a),
        store.publish_project_memory(agent, b)
    );
    assert_ne!(
        a.is_ok(),
        b.is_ok(),
        "only one current successor is permitted"
    );
    assert_eq!(
        store
            .context_memories_page(Some(task), Some(project), Some(agent), false, 20, 0)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        store
            .context_memories_page(Some(task), Some(project), Some(agent), true, 20, 0)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert_eq!(
        store.get_memory_entry(first.id).await.unwrap().content,
        first.content
    );
    let package = store
        .assemble_context_package(task, None, Some(agent))
        .await
        .unwrap();
    assert_eq!(package.memory_refs.len(), 1);
}

#[tokio::test]
async fn project_publication_rejects_stale_context_and_foreign_or_private_sources() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (agent, project, task, context) = fixture(&store).await;
    store
        .claim_task(task, agent, "executor", 300)
        .await
        .unwrap();
    let (_, other_project, other_task, _) = fixture(&store).await;
    let foreign = store
        .create_artifact(
            other_task,
            agent,
            serde_json::from_value(json!({"title":"Other", "uri":"evidence://other"})).unwrap(),
        )
        .await
        .unwrap();
    let mut input = publication(task, context, "bad-ref");
    input.artifact_ids = vec![foreign.id.to_string()];
    assert!(store.publish_project_memory(agent, input).await.is_err());
    for (scope, visibility) in [(other_project, "shared"), (project, "private")] {
        let old=store.create_memory_entry(serde_json::from_value(json!({"scope_type":"project","project_id":scope,"title":"Original", "content":{"keep":true},"source_kind":"operator","visibility":visibility})).unwrap()).await.unwrap();
        let mut input = publication(task, context, &old.id.to_string());
        input.supersedes_memory_id = Some(old.id);
        assert!(store.publish_project_memory(agent, input).await.is_err());
    }
    store
        .update_context_revision(
            task,
            UpdateContextRevision {
                current_summary: Some("Newer".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .publish_project_memory(agent, publication(task, context, "stale"))
            .await
            .unwrap_err()
            .to_string()
            .contains("context changed")
    );
}

#[tokio::test]
async fn assignment_requests_are_durable_deduplicated_and_control_plane_resolved() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (agent, _, task, _) = fixture(&store).await;
    let other = store.register_agent("other", &[]).await.unwrap();
    let (a, b) = tokio::join!(
        store.request_assignment(task, agent, "executor", "I can continue"),
        store.request_assignment(task, agent, "executor", "I can continue")
    );
    let request = a.unwrap();
    assert_eq!(request.id, b.unwrap().id);
    assert!(store.task_assignments(task).await.unwrap().is_empty());
    assert!(store.task_runs(task).await.unwrap().is_empty());
    assert!(
        store
            .resolve_assignment_request(request.id, Some(agent), "approve", "Self", 300)
            .await
            .is_err()
    );
    assert!(
        store
            .resolve_assignment_request(request.id, Some(other.id), "withdraw", "Other", 300)
            .await
            .is_err()
    );
    let approved = store
        .resolve_assignment_request(request.id, None, "approve", "Suitable worker", 300)
        .await
        .unwrap();
    assert_eq!(approved.status, "approved");
    assert_eq!(
        store
            .resolve_assignment_request(request.id, None, "approve", "Retry", 300)
            .await
            .unwrap()
            .assignment_id,
        approved.assignment_id
    );
    assert_eq!(store.task_assignments(task).await.unwrap().len(), 1);
    assert!(store.task_runs(task).await.unwrap().is_empty());
    let waiting = store
        .request_assignment(task, other.id, "executor", "Can help")
        .await
        .unwrap();
    assert!(
        store
            .resolve_assignment_request(waiting.id, None, "approve", "Try", 300)
            .await
            .is_err()
    );
    assert_eq!(
        store
            .get_assignment_request(waiting.id)
            .await
            .unwrap()
            .status,
        "pending"
    );
    let rejected = store
        .resolve_assignment_request(waiting.id, None, "reject", "Executor occupied", 300)
        .await
        .unwrap();
    assert_eq!(rejected.resolution.as_deref(), Some("Executor occupied"));
    assert!(
        store
            .assignment_requests_page(None, None, false, 20, 0)
            .await
            .unwrap()
            .items
            .is_empty()
    );
    let history = store
        .assignment_requests_page(Some(task), None, true, 1, 0)
        .await
        .unwrap();
    assert_eq!(history.next_offset, Some(1));
    let withdrawn = store
        .request_assignment(task, other.id, "reviewer", "Review later")
        .await
        .unwrap();
    assert_eq!(
        store
            .resolve_assignment_request(
                withdrawn.id,
                Some(other.id),
                "withdraw",
                "No longer available",
                300
            )
            .await
            .unwrap()
            .status,
        "withdrawn"
    );
}

async fn valid_completion(store: &Store, run: Id, agent: Id, task: Id) -> Value {
    let artifact = store
        .create_artifact(
            task,
            agent,
            serde_json::from_value(
                json!({"title":"Test evidence", "uri":"simulation://test-evidence"}),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let preview = store
        .run_completion_check(run, agent, &json!({}))
        .await
        .unwrap();
    let mut completion = preview.completion_template;
    for check in completion["checks"].as_array_mut().unwrap() {
        check["status"] = json!("passed");
        check["rationale"] = json!("Fixture assertion passed; simulation only");
        check["artifact_ids"] = json!([artifact.id]);
    }
    json!({"completion":completion,"simulation_only":true})
}

#[tokio::test]
async fn completion_blocks_failed_missing_duplicate_and_foreign_evidence_without_state_changes() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (agent, _, task, _) = fixture(&store).await;
    let assignment = store
        .claim_task(task, agent, "executor", 300)
        .await
        .unwrap();
    let run = store.start_run(assignment.id, agent, None).await.unwrap();
    let other = store.register_agent("other", &[]).await.unwrap();
    assert!(
        store
            .run_completion_check(run.id, other.id, &json!({}))
            .await
            .is_err()
    );
    let missing = store
        .run_completion_check(run.id, agent, &json!({}))
        .await
        .unwrap();
    assert!(!missing.ready);
    assert_eq!(missing.criteria.len(), 2);
    let valid = valid_completion(&store, run.id, agent, task).await;
    let mut duplicate = valid.clone();
    let check = duplicate["completion"]["checks"][0].clone();
    duplicate["completion"]["checks"][1] = check;
    let mut foreign = valid.clone();
    foreign["completion"]["checks"][0]["artifact_ids"] = json!([uuid::Uuid::new_v4()]);
    let mut failed = valid.clone();
    failed["all_acceptance_criteria_met"] = json!(false);
    let before = store.task_events(task).await.unwrap().len();
    for input in [json!({}), duplicate, foreign, failed] {
        assert!(store.complete_run(run.id, agent, input).await.is_err());
        assert_eq!(
            store.get_task(task).await.unwrap().state,
            TaskState::InProgress
        );
        assert_eq!(store.get_run(run.id).await.unwrap().status, "running");
        assert_eq!(
            store.get_assignment(assignment.id).await.unwrap().status,
            "active"
        );
        assert_eq!(store.task_events(task).await.unwrap().len(), before);
    }
    assert!(
        store
            .run_completion_check(run.id, agent, &valid)
            .await
            .unwrap()
            .ready
    );
    store.complete_run(run.id, agent, valid).await.unwrap();
    assert_eq!(store.get_task(task).await.unwrap().state, TaskState::Done);
}

#[tokio::test]
async fn completion_revalidates_after_preflight_and_preserves_structured_criteria() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let (agent, _, task, _) = fixture(&store).await;
    let assignment = store
        .claim_task(task, agent, "executor", 300)
        .await
        .unwrap();
    let run = store.start_run(assignment.id, agent, None).await.unwrap();
    let valid = valid_completion(&store, run.id, agent, task).await;
    assert!(
        store
            .run_completion_check(run.id, agent, &valid)
            .await
            .unwrap()
            .ready
    );
    store
        .update_context_revision(
            task,
            UpdateContextRevision {
                current_summary: Some("New findings".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .complete_run(run.id, agent, valid)
            .await
            .unwrap_err()
            .to_string()
            .contains("stale")
    );
    assert_eq!(store.get_run(run.id).await.unwrap().status, "running");
    let raw = json!({"acceptance_criteria":{"a/b~c":{"required":true,"details":["retain","all"]}}});
    let criteria = completion_criteria(&raw);
    assert_eq!(criteria[0].path, "/acceptance_criteria/a~1b~0c");
    assert_eq!(criteria[0].requirement, raw["acceptance_criteria"]["a/b~c"]);
}
