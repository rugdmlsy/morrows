use super::*;

fn input<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn decoded(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn mcp_dispatch_tools_share_store_semantics() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let profile = store
        .register_profile(input(json!({
            "name":"MCP dispatch","provider":"openai","default_capabilities":["code"]
        })))
        .await
        .unwrap();
    let worker = store
        .register_agent_instance(input(json!({
            "name":"mcp-dispatch-worker","profile_id":profile.id
        })))
        .await
        .unwrap();
    store.agent_heartbeat(worker.id,worker.id,input(json!({
        "status":"online","capacity":{"status":"available","available_slots":1,"active_assignments":0,"active_runs":0,"max_concurrency":1}
    }))).await.unwrap();
    let task = store
        .create_task(input(json!({"title":"MCP dispatch"})))
        .await
        .unwrap();
    let mcp = AgentCompanyMcp::new(store.clone());
    for name in [
        "dispatch_policy_list",
        "dispatch_policy_set",
        "dispatch_policy_get",
        "dispatch_preview",
        "dispatch_task",
        "dispatch_next",
        "dispatch_decisions",
    ] {
        assert!(mcp.tool_router.get(name).is_some(), "missing tool {name}");
    }
    let policy = decoded(
        mcp.dispatch_policy_set(Parameters(DispatchPolicyRequest {
            task_id: task.id.to_string(),
            input: input(
                json!({"role":"executor","required_capabilities":["code"],"lease_seconds":300}),
            ),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(policy["task_id"], json!(task.id));
    assert_eq!(
        decoded(mcp.dispatch_policy_list().await.unwrap())
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let req = || {
        Parameters(TaskRoleRequest {
            task_id: task.id.to_string(),
            role: "executor".into(),
        })
    };
    assert_eq!(
        decoded(mcp.dispatch_policy_get(req()).await.unwrap())["task_id"],
        json!(task.id)
    );
    assert_eq!(
        decoded(mcp.dispatch_preview(req()).await.unwrap())["selected_agent_instance_id"],
        json!(worker.id)
    );
    let outcome = decoded(mcp.dispatch_task(req()).await.unwrap());
    assert_eq!(outcome["assignment"]["agent_instance_id"], json!(worker.id));
    let history = decoded(
        mcp.dispatch_decisions(Parameters(TaskIdRequest {
            task_id: task.id.to_string(),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(history.as_array().unwrap().len(), 1);
}
