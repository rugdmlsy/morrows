use morrows_core::*;
use morrows_store::Store;
use serde_json::{Value, json};
use std::time::Duration as StdDuration;
use tokio::time::sleep;

fn input<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}

async fn worker(
    store: &Store,
    name: &str,
    capabilities: &[&str],
    slots: i64,
    max: i64,
) -> AgentInstance {
    let profile = store
        .register_profile(input(json!({
            "name":format!("{name}-profile"),"provider":"openai",
            "default_capabilities":capabilities
        })))
        .await
        .unwrap();
    let agent = store
        .register_agent_instance(input(json!({
            "name":name,"profile_id":profile.id
        })))
        .await
        .unwrap();
    store
        .agent_heartbeat(
            agent.id,
            agent.id,
            input(json!({
                "status":"online",
                "capacity":{"status":"available","available_slots":slots,"active_assignments":0,"active_runs":0,"max_concurrency":max,"quota_state":"available"}
            })),
        )
        .await
        .unwrap();
    agent
}

async fn policy(store: &Store, task: Id, caps: &[&str]) -> TaskDispatchPolicy {
    store
        .set_dispatch_policy(
            task,
            input(json!({
                "role":"executor","required_capabilities":caps,
                "heartbeat_ttl_seconds":120,"capacity_ttl_seconds":120,
                "lease_seconds":300,"enabled":true
            })),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn preview_explains_identity_capability_capacity_and_task_gates() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let good = worker(&store, "good", &["code", "rust"], 2, 2).await;
    let missing = worker(&store, "missing", &["review"], 1, 1).await;
    let limited = worker(&store, "limited", &["code"], 1, 1).await;
    store
        .agent_heartbeat(
            limited.id,
            limited.id,
            input(json!({"status":"online","capacity":{"status":"throttled","available_slots":1,"active_assignments":0,"active_runs":0,"max_concurrency":1,"quota_state":"usage_limited"}})),
        )
        .await
        .unwrap();
    let task = store
        .create_task(input(json!({"title":"dispatch"})))
        .await
        .unwrap();
    policy(&store, task.id, &["code"]).await;

    let preview = store.dispatch_preview(task.id, "executor").await.unwrap();
    assert_eq!(preview.selected_agent_instance_id, Some(good.id));
    assert!(preview.task_dispatchable);
    let missing_eval = preview
        .candidates
        .iter()
        .find(|c| c.agent_instance_id == missing.id)
        .unwrap();
    assert!(
        missing_eval
            .reasons
            .contains(&"missing_capability:code".into())
    );
    let limited_eval = preview
        .candidates
        .iter()
        .find(|c| c.agent_instance_id == limited.id)
        .unwrap();
    assert!(
        limited_eval
            .reasons
            .iter()
            .any(|r| r.starts_with("capacity_status:"))
    );
    assert!(
        limited_eval
            .reasons
            .iter()
            .any(|r| r.starts_with("quota_state:"))
    );

    let prerequisite = store
        .create_task(input(json!({"title":"dep"})))
        .await
        .unwrap();
    let actor = store.register_agent("actor", &[]).await.unwrap();
    store
        .add_dependency(task.id, prerequisite.id, actor.id)
        .await
        .unwrap();
    let blocked = store.dispatch_preview(task.id, "executor").await.unwrap();
    assert!(!blocked.task_dispatchable);
    assert!(
        blocked
            .task_reasons
            .contains(&"unfinished_dependencies".into())
    );
}

#[tokio::test]
async fn dispatch_compensates_snapshot_slots_and_never_oversubscribes() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = worker(&store, "one-slot", &["code"], 1, 1).await;
    let a = store
        .create_task(input(json!({"title":"a","priority":2})))
        .await
        .unwrap();
    let b = store
        .create_task(input(json!({"title":"b","priority":1})))
        .await
        .unwrap();
    policy(&store, a.id, &["code"]).await;
    policy(&store, b.id, &["code"]).await;

    let first = store.dispatch_task(a.id, "executor").await.unwrap();
    assert_eq!(
        first.assignment.as_ref().map(|v| v.agent_instance_id),
        Some(agent.id)
    );
    let second = store.dispatch_task(b.id, "executor").await.unwrap();
    assert!(second.assignment.is_none());
    let eval = second
        .decision
        .preview
        .candidates
        .iter()
        .find(|c| c.agent_instance_id == agent.id)
        .unwrap();
    assert_eq!(eval.current_active_assignments, 1);
    assert_eq!(eval.effective_slots, 0);
    assert!(eval.reasons.contains(&"no_effective_slots".into()));

    let history = store.task_dispatch_decisions(b.id).await.unwrap();
    assert_eq!(history[0].outcome, "no_candidate");
}

#[tokio::test]
async fn concurrent_dispatch_and_dispatch_next_are_deterministic() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let first_agent = worker(&store, "agent-a", &["code"], 1, 1).await;
    let second_agent = worker(&store, "agent-b", &["code"], 1, 1).await;
    // Effective slots and active count tie here; the freshest capacity observation wins
    // before UUID is considered.
    let high = store
        .create_task(input(json!({"title":"high","priority":50})))
        .await
        .unwrap();
    let low = store
        .create_task(input(json!({"title":"low","priority":10})))
        .await
        .unwrap();
    policy(&store, high.id, &["code"]).await;
    policy(&store, low.id, &["code"]).await;

    let preview = store.dispatch_preview(high.id, "executor").await.unwrap();
    let eligible = preview
        .candidates
        .iter()
        .filter(|c| c.eligible)
        .collect::<Vec<_>>();
    assert_eq!(eligible.len(), 2);
    let expected = eligible[0].agent_instance_id;
    assert_eq!(preview.selected_agent_instance_id, Some(expected));
    assert!([first_agent.id, second_agent.id].contains(&expected));

    let next = store.dispatch_next("executor").await.unwrap();
    assert_eq!(next.dispatched.as_ref().unwrap().decision.task_id, high.id);
    assert_eq!(
        next.dispatched
            .as_ref()
            .unwrap()
            .assignment
            .as_ref()
            .unwrap()
            .agent_instance_id,
        expected
    );

    // Two one-slot agents remain globally capacity-safe when two task dispatches race.
    let t1 = store
        .create_task(input(json!({"title":"race-1"})))
        .await
        .unwrap();
    let t2 = store
        .create_task(input(json!({"title":"race-2"})))
        .await
        .unwrap();
    policy(&store, t1.id, &["code"]).await;
    policy(&store, t2.id, &["code"]).await;
    let s1 = store.clone();
    let s2 = store.clone();
    let (r1, r2) = tokio::join!(
        async move { s1.dispatch_task(t1.id, "executor").await.unwrap() },
        async move { s2.dispatch_task(t2.id, "executor").await.unwrap() }
    );
    let assigned = [r1.assignment, r2.assignment]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    // One agent is occupied by "high"; only the other one-slot agent may accept one race task.
    assert_eq!(assigned.len(), 1);
    assert_ne!(assigned[0].agent_instance_id, expected);

    // Current DB assignment count, not an old capacity observation, removes its slot.
    let remaining = store.dispatch_preview(low.id, "executor").await.unwrap();
    assert!(remaining.selected_agent_instance_id.is_none());
}

#[tokio::test]
async fn scheduled_dispatch_is_persistent_and_does_not_spam_no_candidate_decisions() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let defaults = store
        .get_dispatch_scheduler_settings("executor")
        .await
        .unwrap();
    assert!(!defaults.enabled);
    assert_eq!(defaults.interval_seconds, 2);
    assert!(defaults.auto_launch);

    let configured = store
        .set_dispatch_scheduler_settings(
            "executor",
            SetDispatchSchedulerSettings {
                enabled: true,
                interval_seconds: 1,
                auto_launch: false,
            },
        )
        .await
        .unwrap();
    assert!(configured.enabled);
    assert_eq!(configured.interval_seconds, 1);
    assert!(!configured.auto_launch);
    assert_eq!(
        store
            .list_dispatch_scheduler_settings()
            .await
            .unwrap()
            .len(),
        1
    );

    let task = store
        .create_task(input(json!({"title":"no scheduler candidate"})))
        .await
        .unwrap();
    policy(&store, task.id, &["missing-capability"]).await;

    for _ in 0..3 {
        let result = store.dispatch_next_scheduled("executor").await.unwrap();
        assert!(result.dispatched.is_none());
        assert!(result.attempts.is_empty());
    }
    assert!(
        store
            .task_dispatch_decisions(task.id)
            .await
            .unwrap()
            .is_empty()
    );

    let manual = store.dispatch_next("executor").await.unwrap();
    assert!(manual.dispatched.is_none());
    assert_eq!(manual.attempts.len(), 1);
    assert_eq!(
        store.task_dispatch_decisions(task.id).await.unwrap().len(),
        1
    );

    assert!(matches!(
        store
            .set_dispatch_scheduler_settings(
                "reviewer",
                SetDispatchSchedulerSettings {
                    enabled: true,
                    interval_seconds: 1,
                    auto_launch: true,
                },
            )
            .await,
        Err(DomainError::InvalidInput(_))
    ));
}

#[tokio::test]
async fn stale_capacity_and_policy_filters_are_rejected() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = worker(&store, "filtered", &["code"], 1, 1).await;
    let task = store
        .create_task(input(json!({"title":"filters"})))
        .await
        .unwrap();
    store
        .set_dispatch_policy(
            task.id,
            input(json!({
                "role":"executor","required_capabilities":["code"],
                "profile_id":agent.profile_id,
                "heartbeat_ttl_seconds":1,"capacity_ttl_seconds":1,
                "lease_seconds":30,"enabled":true
            })),
        )
        .await
        .unwrap();
    sleep(StdDuration::from_millis(2100)).await;
    let preview = store.dispatch_preview(task.id, "executor").await.unwrap();
    let candidate = preview
        .candidates
        .iter()
        .find(|c| c.agent_instance_id == agent.id)
        .unwrap();
    assert!(candidate.reasons.contains(&"stale_heartbeat".into()));
    assert!(candidate.reasons.contains(&"stale_capacity".into()));

    store
        .agent_heartbeat(
            agent.id,
            agent.id,
            input(json!({
                "status":"online",
                "capacity":{"status":"available","available_slots":1,"active_assignments":0,"active_runs":0,"max_concurrency":1}
            })),
        )
        .await
        .unwrap();
    let account = store
        .register_account(input(json!({"provider":"openai","label":"other-account"})))
        .await
        .unwrap();
    let machine = store
        .register_machine(input(json!({"name":"other-machine"})))
        .await
        .unwrap();
    store
        .set_dispatch_policy(
            task.id,
            input(json!({
                "role":"executor","required_capabilities":["code"],
                "profile_id":agent.profile_id,"account_id":account.id,"machine_id":machine.id,
                "heartbeat_ttl_seconds":120,"capacity_ttl_seconds":120,
                "lease_seconds":30,"enabled":true
            })),
        )
        .await
        .unwrap();
    let filtered = store.dispatch_preview(task.id, "executor").await.unwrap();
    let candidate = filtered
        .candidates
        .iter()
        .find(|c| c.agent_instance_id == agent.id)
        .unwrap();
    assert!(candidate.reasons.contains(&"account_mismatch".into()));
    assert!(candidate.reasons.contains(&"machine_mismatch".into()));

    let disabled = store
        .set_dispatch_policy(
            task.id,
            input(json!({
                "role":"executor","required_capabilities":["code"],
                "heartbeat_ttl_seconds":120,"capacity_ttl_seconds":120,
                "lease_seconds":30,"enabled":false
            })),
        )
        .await
        .unwrap();
    assert!(!disabled.enabled);
    let preview = store.dispatch_preview(task.id, "executor").await.unwrap();
    assert!(preview.task_reasons.contains(&"policy_disabled".into()));
}

#[tokio::test]
async fn dispatch_decisions_are_append_only_in_sqlite() {
    let path = std::env::temp_dir().join(format!("ac-dispatch-audit-{}.db", uuid::Uuid::new_v4()));
    let url = format!("sqlite://{}", path.display());
    let store = Store::connect(&url).await.unwrap();
    let agent = worker(&store, "audit-worker", &["code"], 1, 1).await;
    let task = store
        .create_task(input(json!({"title":"append-only decision"})))
        .await
        .unwrap();
    policy(&store, task.id, &["code"]).await;
    let outcome = store.dispatch_task(task.id, "executor").await.unwrap();
    let decision_id = outcome.decision.id;

    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    let update = sqlx::query("UPDATE dispatch_decisions SET outcome='tampered' WHERE id=?")
        .bind(decision_id.to_string())
        .execute(&pool)
        .await;
    assert!(update.is_err());
    let delete = sqlx::query("DELETE FROM dispatch_decisions WHERE id=?")
        .bind(decision_id.to_string())
        .execute(&pool)
        .await;
    assert!(delete.is_err());
    assert_eq!(
        store.task_dispatch_decisions(task.id).await.unwrap()[0].outcome,
        "assigned"
    );
    pool.close().await;
    drop(store);
    let _ = std::fs::remove_file(path);
    assert_eq!(outcome.assignment.unwrap().agent_instance_id, agent.id);
}

#[tokio::test]
async fn continuation_chain_is_ordered_single_owner_and_recovers_checkpoints() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = worker(&store, "chain-a", &["code"], 1, 1).await;
    let b = worker(&store, "chain-b", &["code"], 1, 1).await;
    let c = worker(&store, "chain-c", &["code"], 1, 1).await;
    let outsider = worker(&store, "outsider", &["code"], 9, 9).await;
    let task = store
        .create_task(input(json!({"title":"one task three executions"})))
        .await
        .unwrap();
    let context = store
        .create_context_revision(
            task.id,
            input(json!({
                "goal":"continue", "background":"durable source", "constraints":{},
                "current_summary":"initial", "created_by_actor_id":"human:test"
            })),
        )
        .await
        .unwrap();
    policy(&store, task.id, &["code"]).await;
    store
        .set_task_continuation_policy(
            task.id,
            TaskContinuationPolicy {
                enabled: true,
                agent_ids: vec![a.id, b.id, c.id],
            },
        )
        .await
        .unwrap();
    let mut previous = None;
    for agent in [a.id, b.id, c.id] {
        // Racing schedulers must create exactly one assignment at each transition.
        let (one, two) = tokio::join!(
            store.dispatch_task(task.id, "executor"),
            store.dispatch_task(task.id, "executor")
        );
        let assignments: Vec<_> = [one.unwrap().assignment, two.unwrap().assignment]
            .into_iter()
            .flatten()
            .collect();
        assert_eq!(assignments.len(), 1);
        let assignment = &assignments[0];
        assert_eq!(assignment.agent_instance_id, agent);
        assert_eq!(assignment.task_id, task.id);
        let (one, two) = tokio::join!(
            store.start_run(assignment.id, agent, None),
            store.start_run(assignment.id, agent, None)
        );
        assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
        let run = one.or(two).unwrap();
        assert_eq!(run.task_id, task.id);
        if let Some((old_run, old_agent, handoff)) = previous {
            let recovery = store.task_recovery_context(task.id, agent).await.unwrap();
            assert_eq!(recovery["source_run_id"], json!(old_run));
            assert_eq!(
                recovery["checkpoint"],
                json!({"next":"continue","agent":old_agent})
            );
            assert_eq!(recovery["context_revision_id"], json!(context.id));
            assert!(recovery["checkpoint_at"].is_string());
            assert_eq!(recovery["handoff_id"], json!(handoff));
            store.accept_handoff(handoff, run.id, agent).await.unwrap();
            assert!(
                store
                    .checkpoint_run(old_run, old_agent, json!({"stale":true}))
                    .await
                    .is_err()
            );
            assert!(
                store
                    .complete_run(old_run, old_agent, json!({}))
                    .await
                    .is_err()
            );
        }
        assert_eq!(
            store
                .task_recovery_context(task.id, outsider.id)
                .await
                .unwrap()["available"],
            false
        );
        store
            .checkpoint_run(run.id, agent, json!({"next":"continue","agent":agent}))
            .await
            .unwrap();
        let credential = store
            .issue_runtime_credential(agent, run.id, "chain test", 600)
            .await
            .unwrap();
        store.verify_agent_token(&credential.token).await.unwrap();
        let handoff = store.create_handoff(run.id,agent,input(json!({"summary":"provider budget exhausted","completed":[],"remaining":["continue"],"blockers":[],"artifact_ids":[],"decision_ids":[]}))).await.unwrap();
        assert!(store.verify_agent_token(&credential.token).await.is_err());
        previous = Some((run.id, agent, handoff.id));
    }
    // Exhaustion waits even though all workers report spare capacity.
    let preview = store.dispatch_preview(task.id, "executor").await.unwrap();
    assert!(preview.selected_agent_instance_id.is_none());
    assert!(
        store
            .dispatch_task(task.id, "executor")
            .await
            .unwrap()
            .assignment
            .is_none()
    );
    assert_ne!(
        store.get_task(task.id).await.unwrap().state,
        TaskState::Done
    );
}

#[tokio::test]
async fn continuation_waits_for_unavailable_candidates_and_validates_policy() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = worker(&store, "unavailable", &["code"], 0, 1).await;
    let task = store
        .create_task(input(json!({"title":"wait"})))
        .await
        .unwrap();
    policy(&store, task.id, &["code"]).await;
    for ids in [vec![], vec![agent.id, agent.id], vec![uuid::Uuid::new_v4()]] {
        assert!(
            store
                .set_task_continuation_policy(
                    task.id,
                    TaskContinuationPolicy {
                        enabled: true,
                        agent_ids: ids
                    }
                )
                .await
                .is_err()
        );
    }
    store
        .set_task_continuation_policy(
            task.id,
            TaskContinuationPolicy {
                enabled: true,
                agent_ids: vec![agent.id],
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .dispatch_task(task.id, "executor")
            .await
            .unwrap()
            .assignment
            .is_none()
    );
    store.agent_heartbeat(agent.id,agent.id,input(json!({"status":"online","capacity":{"status":"available","available_slots":1,"active_assignments":0,"active_runs":0,"max_concurrency":1,"quota_state":"available"}}))).await.unwrap();
    assert_eq!(
        store
            .dispatch_task(task.id, "executor")
            .await
            .unwrap()
            .assignment
            .unwrap()
            .agent_instance_id,
        agent.id
    );
}
