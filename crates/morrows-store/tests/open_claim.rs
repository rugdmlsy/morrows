use morrows_core::*;
use morrows_store::Store;
use serde_json::{Value, json};

fn input<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}

async fn agent(store: &Store, name: &str) -> AgentInstance {
    store.register_agent(name, &[]).await.unwrap()
}

#[tokio::test]
async fn open_task_claim_atomically_creates_assignment_and_run() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "worker").await;
    let task = store.create_task(input(json!({"title":"open work"}))).await.unwrap();
    assert_eq!(task.assignment_mode, AssignmentMode::Open);
    let claim = store.claim_task_for_execution(task.id, worker.id, "executor", 300).await.unwrap();
    assert_eq!(claim.assignment.task_id, task.id);
    assert_eq!(claim.assignment.agent_instance_id, worker.id);
    assert_eq!(claim.assignment.status, "active");
    assert_eq!(claim.run.assignment_id, claim.assignment.id);
    assert_eq!(claim.run.agent_instance_id, worker.id);
    assert_eq!(claim.run.status, "running");
    assert_eq!(store.get_task(task.id).await.unwrap().state, TaskState::InProgress);
    assert_eq!(store.task_runs(task.id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn open_task_claim_requires_ready_state() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "worker").await;
    let task = store
        .create_task(input(json!({"title":"blocked","state":"blocked"})))
        .await
        .unwrap();
    let error = store
        .claim_task_for_execution(task.id, worker.id, "executor", 300)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("only ready tasks can be self-claimed"));
    assert!(store.task_assignments(task.id).await.unwrap().is_empty());
    assert!(store.task_runs(task.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn concurrent_open_claim_has_one_winner_and_one_run() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = agent(&store, "a").await;
    let b = agent(&store, "b").await;
    let task = store.create_task(input(json!({"title":"race"}))).await.unwrap();
    let left = store.clone();
    let right = store.clone();
    let (one, two) = tokio::join!(
        left.claim_task_for_execution(task.id, a.id, "executor", 300),
        right.claim_task_for_execution(task.id, b.id, "executor", 300)
    );
    assert_ne!(one.is_ok(), two.is_ok());
    let error = if let Err(error) = one { error } else { two.unwrap_err() };
    assert!(
        error.to_string().contains("already has an active assignment")
            || error.to_string().contains("only ready tasks can be self-claimed")
    );
    assert_eq!(store.task_runs(task.id).await.unwrap().len(), 1);
    assert_eq!(
        store.task_assignments(task.id).await.unwrap().into_iter()
            .filter(|assignment| assignment.status == "active").count(),
        1
    );
}

#[tokio::test]
async fn approval_and_dispatch_modes_reject_self_claim() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "worker").await;
    let approval = store.create_task(input(json!({"title":"approval"}))).await.unwrap();
    store.set_task_assignment_mode(approval.id, AssignmentMode::Approval).await.unwrap();
    assert!(store.claim_task_for_execution(approval.id, worker.id, "executor", 300).await.unwrap_err().to_string().contains("requires assignment approval"));
    let request = store.request_assignment(approval.id, worker.id, "executor", "ready to execute").await.unwrap();
    let approved = store.resolve_assignment_request(request.id, None, "approve", "approved", 300).await.unwrap();
    assert_eq!(approved.status, "approved");
    assert!(approved.assignment_id.is_some());

    let dispatch = store.create_task(input(json!({"title":"dispatch"}))).await.unwrap();
    store.set_task_assignment_mode(dispatch.id, AssignmentMode::Dispatch).await.unwrap();
    assert!(store.claim_task_for_execution(dispatch.id, worker.id, "executor", 300).await.unwrap_err().to_string().contains("dispatcher-managed"));
    assert!(store.request_assignment(dispatch.id, worker.id, "executor", "want it").await.unwrap_err().to_string().contains("dispatcher-managed"));
}

#[tokio::test]
async fn switching_approval_task_to_open_resolves_existing_request_on_claim() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = agent(&store, "worker").await;
    let task = store.create_task(input(json!({"title":"cutover"}))).await.unwrap();
    store.set_task_assignment_mode(task.id, AssignmentMode::Approval).await.unwrap();
    let request = store.request_assignment(task.id, worker.id, "executor", "existing request").await.unwrap();
    store.set_task_assignment_mode(task.id, AssignmentMode::Open).await.unwrap();
    let claim = store.claim_task_for_execution(task.id, worker.id, "executor", 300).await.unwrap();
    let resolved = store.get_assignment_request(request.id).await.unwrap();
    assert_eq!(resolved.status, "approved");
    assert_eq!(resolved.assignment_id, Some(claim.assignment.id));
    assert_eq!(resolved.resolution.as_deref(), Some("auto-approved by open task claim"));
}

#[tokio::test]
async fn enabling_dispatch_policy_marks_task_dispatch_owned() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let task = store.create_task(input(json!({"title":"scheduled"}))).await.unwrap();
    assert_eq!(task.assignment_mode, AssignmentMode::Open);
    store.set_dispatch_policy(
        task.id,
        input(json!({
            "role":"executor","required_capabilities":[],
            "heartbeat_ttl_seconds":120,"capacity_ttl_seconds":120,
            "lease_seconds":300,"enabled":true
        })),
    ).await.unwrap();
    assert_eq!(store.get_task(task.id).await.unwrap().assignment_mode, AssignmentMode::Dispatch);

    store
        .set_task_assignment_mode(task.id, AssignmentMode::Open)
        .await
        .unwrap();
    let preview = store.dispatch_preview(task.id, "executor").await.unwrap();
    assert!(!preview.task_dispatchable);
    assert!(
        preview
            .task_reasons
            .contains(&"assignment_mode:open".to_owned())
    );
}
