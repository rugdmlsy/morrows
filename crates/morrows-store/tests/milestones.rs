use morrows_core::*;
use morrows_store::Store;
use serde_json::json;

async fn setup() -> (Store, AgentInstance, Task, ContextRevision, Assignment, Run) {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let agent = store.register_agent("worker", &[]).await.unwrap();
    let task = store
        .create_task(
            serde_json::from_value(json!({
                "title":"Milestone recovery",
                "state":"ready"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let context = store
        .create_context_revision(
            task.id,
            serde_json::from_value(json!({
                "goal":"preserve progress",
                "background":"test",
                "current_summary":"phase one",
                "constraints":{},
                "created_by_actor_id":"human:test"
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    let assignment = store
        .claim_task(task.id, agent.id, "executor", 300)
        .await
        .unwrap();
    let run = store
        .start_run(assignment.id, agent.id, None)
        .await
        .unwrap();
    (store, agent, task, context, assignment, run)
}

fn milestone_input(artifact_ids: Vec<String>, decision_ids: Vec<String>) -> CreateRunMilestone {
    CreateRunMilestone {
        kind: "milestone".into(),
        summary: "phase boundary".into(),
        completed: vec!["phase one".into()],
        verified: vec!["tests pass".into()],
        remaining: vec!["phase two".into()],
        blockers: vec![],
        next_step: "start phase two".into(),
        next_plan: vec!["start phase two".into(), "verify phase two".into()],
        execution_locations: vec!["/workspace/project".into()],
        artifact_ids,
        decision_ids,
    }
}

fn handoff_input(milestone: Id, artifact_ids: Vec<String>) -> CreateHandoff {
    CreateHandoff {
        milestone_id: Some(milestone.to_string()),
        summary: "continue from durable milestone".into(),
        completed: vec!["phase one".into()],
        remaining: vec!["phase two".into()],
        blockers: vec![],
        artifact_ids,
        decision_ids: vec![],
    }
}

#[tokio::test]
async fn new_milestones_require_a_nonempty_ordered_next_plan() {
    let (store, agent, _, _, _, run) = setup().await;
    let mut input = milestone_input(vec![], vec![]);
    input.next_plan.clear();
    let error = store
        .create_run_milestone(run.id, agent.id, input)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("next_plan"));

    let mut input = milestone_input(vec![], vec![]);
    input.next_plan = vec![" ".into()];
    let error = store
        .create_run_milestone(run.id, agent.id, input)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("milestone item"));
}

#[tokio::test]
async fn milestones_are_immutable_sequence_and_refresh_latest_checkpoint() {
    let (store, agent, _, context, _, run) = setup().await;
    let first = store
        .create_run_milestone(run.id, agent.id, milestone_input(vec![], vec![]))
        .await
        .unwrap();
    let mut second_input = milestone_input(vec![], vec![]);
    second_input.summary = "second durable boundary".into();
    second_input.next_step = "handoff or continue".into();
    let second = store
        .create_run_milestone(run.id, agent.id, second_input)
        .await
        .unwrap();

    assert_eq!(first.sequence, 1);
    assert_eq!(second.sequence, 2);
    assert_eq!(first.context_revision_id, Some(context.id));
    let all = store.run_milestones(run.id).await.unwrap();
    assert_eq!(all.len(), 2);
    assert_eq!(all[0].id, first.id);
    assert_eq!(all[1].id, second.id);

    let saved = store.get_run(run.id).await.unwrap();
    let checkpoint = saved.checkpoint.unwrap();
    assert_eq!(checkpoint["milestone_id"], json!(second.id));
    assert_eq!(checkpoint["sequence"], 2);
    assert_eq!(checkpoint["next_step"], "handoff or continue");
}

#[tokio::test]
async fn handoff_rejects_stale_context_and_evidence_after_milestone_until_refreshed() {
    let (store, agent, task, _, _, run) = setup().await;
    let first = store
        .create_run_milestone(run.id, agent.id, milestone_input(vec![], vec![]))
        .await
        .unwrap();

    store
        .update_context_revision(
            task.id,
            UpdateContextRevision {
                current_summary: Some("phase one completed; phase two pending".into()),
                expected_context_revision_id: first.context_revision_id,
                created_by_actor_id: format!("agent:{}", agent.id),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let stale = store
        .create_handoff(run.id, agent.id, handoff_input(first.id, vec![]))
        .await
        .unwrap_err();
    assert!(
        stale
            .to_string()
            .contains("stale relative to current task context")
    );

    let second = store
        .create_run_milestone(run.id, agent.id, milestone_input(vec![], vec![]))
        .await
        .unwrap();
    store
        .checkpoint_run(
            run.id,
            agent.id,
            json!({"note":"progress after milestone that must be consolidated"}),
        )
        .await
        .unwrap();
    let overwritten = store
        .create_handoff(run.id, agent.id, handoff_input(second.id, vec![]))
        .await
        .unwrap_err();
    assert!(
        overwritten
            .to_string()
            .contains("fresh milestone after the Run's latest checkpoint")
    );

    let third = store
        .create_run_milestone(run.id, agent.id, milestone_input(vec![], vec![]))
        .await
        .unwrap();
    let artifact = store
        .create_artifact(
            task.id,
            agent.id,
            serde_json::from_value(json!({
                "title":"late evidence",
                "uri":"file:///tmp/evidence",
                "kind":"test"
            }))
            .unwrap(),
        )
        .await
        .unwrap();

    let late = store
        .create_handoff(
            run.id,
            agent.id,
            handoff_input(third.id, vec![artifact.id.to_string()]),
        )
        .await
        .unwrap_err();
    assert!(late.to_string().contains("after the cited milestone"));

    let fourth = store
        .create_run_milestone(
            run.id,
            agent.id,
            milestone_input(vec![artifact.id.to_string()], vec![]),
        )
        .await
        .unwrap();
    let handoff = store
        .create_handoff(
            run.id,
            agent.id,
            handoff_input(fourth.id, vec![artifact.id.to_string()]),
        )
        .await
        .unwrap();
    assert_eq!(handoff.milestone_id, Some(fourth.id));
    assert_eq!(
        store
            .get_handoff(handoff.id)
            .await
            .unwrap()
            .milestone
            .unwrap()
            .id,
        fourth.id
    );
}
