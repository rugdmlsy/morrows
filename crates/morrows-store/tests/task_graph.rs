use morrows_core::*;
use morrows_store::Store;
use serde_json::json;

async fn task(store: &Store, title: &str) -> Task {
    store
        .create_task(CreateTask {
            project_id: None,
            title: title.into(),
            description: String::new(),
            owner_actor_id: "human:test".into(),
            state: TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap()
}

async fn chain(store: &Store, name: &str) -> TaskChain {
    store
        .save_task_collection(
            true,
            None,
            SaveTaskCollection {
                name: name.into(),
                description: String::new(),
                archived: false,
            },
        )
        .await
        .unwrap()
}

async fn member(store: &Store, chain_id: Id, task_id: Id, join: JoinMode) {
    store
        .set_collection_member(true, chain_id, task_id, join, false)
        .await
        .unwrap();
}

async fn edge(
    store: &Store,
    chain_id: Id,
    predecessor: Id,
    successor: Id,
    condition: EdgeCondition,
) {
    store
        .save_chain_edge(
            chain_id,
            SaveChainEdge {
                task_id: successor,
                depends_on_task_id: predecessor,
                condition,
            },
        )
        .await
        .unwrap();
}

async fn complete(store: &Store, task_id: Id, agent: Id, result: serde_json::Value) {
    let assignment = store
        .claim_task(task_id, agent, "executor", 300)
        .await
        .unwrap();
    let run = store.start_run(assignment.id, agent, None).await.unwrap();
    store.complete_run(run.id, agent, result).await.unwrap();
}

#[tokio::test]
async fn conditional_branch_selects_one_successor_and_excludes_the_other() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = store.register_agent("chain-worker", &[]).await.unwrap();
    let a = task(&store, "A").await;
    let b = task(&store, "B").await;
    let c = task(&store, "C").await;
    let d = task(&store, "D below excluded C").await;
    let chain = chain(&store, "branch").await;
    for id in [a.id, b.id, c.id, d.id] {
        member(&store, chain.id, id, JoinMode::All).await;
    }
    edge(
        &store,
        chain.id,
        a.id,
        b.id,
        EdgeCondition::ResultEquals {
            path: "/kind".into(),
            value: json!("x"),
        },
    )
    .await;
    edge(
        &store,
        chain.id,
        a.id,
        c.id,
        EdgeCondition::ResultEquals {
            path: "/kind".into(),
            value: json!("y"),
        },
    )
    .await;
    edge(&store, chain.id, c.id, d.id, EdgeCondition::Unconditional).await;

    assert_eq!(
        store.task_gate(b.id).await.unwrap().state,
        GateState::Waiting
    );
    assert_eq!(
        store.task_gate(c.id).await.unwrap().state,
        GateState::Waiting
    );
    complete(&store, a.id, worker.id, json!({"kind":"x"})).await;

    let b_gate = store.task_gate(b.id).await.unwrap();
    assert_eq!(b_gate.state, GateState::Eligible);
    let c_gate = store.task_gate(c.id).await.unwrap();
    assert_eq!(c_gate.state, GateState::Excluded);
    assert_eq!(
        c_gate.predecessors[0].reason,
        "result_condition_not_matched"
    );
    let d_gate = store.task_gate(d.id).await.unwrap();
    assert_eq!(d_gate.state, GateState::Excluded);
    assert_eq!(d_gate.predecessors[0].reason, "predecessor_branch_excluded");

    store
        .claim_task(b.id, worker.id, "executor", 300)
        .await
        .unwrap();
    assert!(matches!(
        store.claim_task(c.id, worker.id, "executor", 300).await,
        Err(DomainError::Conflict(_))
    ));
}

#[tokio::test]
async fn all_and_any_join_modes_resolve_multiple_predecessors() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = store.register_agent("join-worker", &[]).await.unwrap();
    let a = task(&store, "A").await;
    let b = task(&store, "B").await;
    let target = task(&store, "target").await;
    let chain = chain(&store, "join").await;
    for id in [a.id, b.id, target.id] {
        member(&store, chain.id, id, JoinMode::All).await;
    }
    edge(
        &store,
        chain.id,
        a.id,
        target.id,
        EdgeCondition::Unconditional,
    )
    .await;
    edge(
        &store,
        chain.id,
        b.id,
        target.id,
        EdgeCondition::Unconditional,
    )
    .await;

    complete(&store, a.id, worker.id, json!({"ok":true})).await;
    assert_eq!(
        store.task_gate(target.id).await.unwrap().state,
        GateState::Waiting
    );

    store
        .set_collection_member(true, chain.id, target.id, JoinMode::Any, false)
        .await
        .unwrap();
    assert_eq!(
        store.task_gate(target.id).await.unwrap().state,
        GateState::Eligible
    );

    store
        .set_collection_member(true, chain.id, target.id, JoinMode::All, false)
        .await
        .unwrap();
    complete(&store, b.id, worker.id, json!({"ok":true})).await;
    assert_eq!(
        store.task_gate(target.id).await.unwrap().state,
        GateState::Eligible
    );
}

#[tokio::test]
async fn cancelled_predecessor_excludes_successor() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = task(&store, "cancelled A").await;
    let b = task(&store, "B").await;
    let chain = chain(&store, "cancel").await;
    member(&store, chain.id, a.id, JoinMode::All).await;
    member(&store, chain.id, b.id, JoinMode::All).await;
    edge(&store, chain.id, a.id, b.id, EdgeCondition::Unconditional).await;

    store.cancel_task(a.id).await.unwrap();
    let gate = store.task_gate(b.id).await.unwrap();
    assert_eq!(gate.state, GateState::Excluded);
    assert_eq!(gate.predecessors[0].reason, "predecessor_cancelled");
}

#[tokio::test]
async fn executor_run_start_rechecks_gate_after_assignment() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = store.register_agent("late-gate-worker", &[]).await.unwrap();
    let prerequisite = task(&store, "late prerequisite").await;
    let target = task(&store, "already assigned").await;
    let assignment = store
        .claim_task(target.id, worker.id, "executor", 300)
        .await
        .unwrap();
    store
        .add_dependency(target.id, prerequisite.id, worker.id)
        .await
        .unwrap();
    assert!(matches!(
        store.start_run(assignment.id, worker.id, None).await,
        Err(DomainError::Conflict(_))
    ));
}

#[tokio::test]
async fn task_group_batch_assignment_keeps_members_independent_and_reports_blocked() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let worker = store.register_agent("group-worker", &[]).await.unwrap();
    let free_a = task(&store, "free A").await;
    let free_b = task(&store, "free B").await;
    let prerequisite = task(&store, "prerequisite").await;
    let blocked = task(&store, "blocked").await;
    store
        .add_dependency(blocked.id, prerequisite.id, worker.id)
        .await
        .unwrap();

    let group = store
        .save_task_collection(
            false,
            None,
            SaveTaskCollection {
                name: "mixed".into(),
                description: "independent + dependency-gated".into(),
                archived: false,
            },
        )
        .await
        .unwrap();
    for id in [free_a.id, free_b.id, blocked.id] {
        store
            .set_collection_member(false, group.id, id, JoinMode::All, false)
            .await
            .unwrap();
    }

    let results = store
        .assign_task_group(GroupBatchAssign {
            group_id: group.id,
            agent_instance_id: worker.id,
            lease_seconds: 300,
        })
        .await
        .unwrap();
    assert_eq!(results.len(), 3);
    assert_eq!(results.iter().filter(|r| r.status == "assigned").count(), 2);
    assert_eq!(results.iter().filter(|r| r.status == "blocked").count(), 1);

    let a_assignments = store.task_assignments(free_a.id).await.unwrap();
    let b_assignments = store.task_assignments(free_b.id).await.unwrap();
    assert_eq!(a_assignments.len(), 1);
    assert_eq!(b_assignments.len(), 1);
    assert_ne!(a_assignments[0].id, b_assignments[0].id);
    assert_eq!(store.task_assignments(blocked.id).await.unwrap().len(), 0);
}

#[tokio::test]
async fn chain_detail_preserves_operator_provenance_without_fake_agent() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = task(&store, "A").await;
    let b = task(&store, "B").await;
    let chain = chain(&store, "operator provenance").await;
    member(&store, chain.id, a.id, JoinMode::All).await;
    member(&store, chain.id, b.id, JoinMode::All).await;
    edge(&store, chain.id, a.id, b.id, EdgeCondition::Unconditional).await;

    let dependencies = store.task_dependencies(b.id).await.unwrap();
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].created_by, None);
    assert_eq!(
        dependencies[0].created_by_actor_id,
        "operator:control-plane"
    );
}

#[tokio::test]
async fn agent_owned_groups_respect_owner_and_task_write_access() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let owner = store.register_agent("group-owner", &[]).await.unwrap();
    let other = store.register_agent("other-owner", &[]).await.unwrap();
    let own = store
        .create_task(CreateTask {
            project_id: None,
            title: "own".into(),
            description: String::new(),
            owner_actor_id: format!("agent:{}", owner.id),
            state: TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap();
    let foreign = store
        .create_task(CreateTask {
            project_id: None,
            title: "foreign".into(),
            description: String::new(),
            owner_actor_id: format!("agent:{}", other.id),
            state: TaskState::Ready,
            priority: 0,
        })
        .await
        .unwrap();

    let group = store
        .create_agent_task_group(
            owner.id,
            SaveTaskCollection {
                name: "  Agent Group  ".into(),
                description: "bounded".into(),
                archived: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(group.name, "Agent Group");
    assert!(
        store
            .change_agent_task_group_member(owner.id, group.id, own.id, false)
            .await
            .is_ok()
    );
    assert!(
        store
            .change_agent_task_group_member(owner.id, group.id, own.id, false)
            .await
            .is_ok()
    );
    assert_eq!(
        store.task_collection_detail(false, group.id).await.unwrap()["members"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    assert!(
        store
            .change_agent_task_group_member(owner.id, group.id, foreign.id, false)
            .await
            .is_err()
    );
    assert!(
        store
            .change_agent_task_group_member(other.id, group.id, own.id, false)
            .await
            .is_err()
    );
    assert!(
        store
            .change_agent_task_group_member(other.id, group.id, own.id, true)
            .await
            .is_err()
    );

    let old = store
        .save_task_collection(
            false,
            None,
            SaveTaskCollection {
                name: "Operator legacy".into(),
                description: String::new(),
                archived: false,
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .change_agent_task_group_member(owner.id, old.id, own.id, false)
            .await
            .is_err()
    );

    store
        .change_agent_task_group_member(owner.id, group.id, own.id, true)
        .await
        .unwrap();
    assert!(
        store.task_collection_detail(false, group.id).await.unwrap()["members"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    store
        .archive_task_collection(false, group.id)
        .await
        .unwrap();
    assert!(
        store
            .change_agent_task_group_member(owner.id, group.id, own.id, false)
            .await
            .is_err()
    );
}
