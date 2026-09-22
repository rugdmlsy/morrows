use super::*;

fn input<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn decoded(raw: String) -> Value {
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn mcp_launch_tools_cover_profile_enqueue_and_task_history() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let profile = store
        .register_profile(input(json!({
            "name":"MCP launcher agent","provider":"local","default_capabilities":["code"]
        })))
        .await
        .unwrap();
    let agent = store
        .register_agent_instance(input(json!({
            "name":"mcp-launch-agent","profile_id":profile.id,"capabilities":["code"]
        })))
        .await
        .unwrap();
    let task = store
        .create_task(input(json!({"title":"MCP launch"})))
        .await
        .unwrap();
    let assignment = store
        .claim_task(task.id, agent.id, "executor", 600)
        .await
        .unwrap();
    let mcp = MorrowsMcp::new(store.clone());

    for name in [
        "launch_profile_register",
        "launch_profile_list",
        "launch_profile_get",
        "launch_enqueue",
        "launch_attempt_get",
        "task_launch_attempts",
        "external_launch_list",
        "external_launch_accept",
        "launch_instruction_send",
        "launch_instructions",
        "launch_stop",
    ] {
        assert!(mcp.tool_router.get(name).is_some(), "missing tool {name}");
    }

    let launch_profile = decoded(
        mcp.launch_profile_register(Parameters(RegisterLaunchProfileRequest {
            input: input(json!({
                "name":"mcp fake codex",
                "adapter":"codex_cli",
                "agent_instance_id":agent.id,
                "program":"/bin/echo",
                "default_cwd":std::env::temp_dir(),
                "enabled":true
            })),
        }))
        .await
        .unwrap(),
    );
    let launch_profile_id = launch_profile["id"].as_str().unwrap().to_owned();
    assert_eq!(
        decoded(mcp.launch_profile_list().await.unwrap())
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        decoded(
            mcp.launch_profile_get(Parameters(LaunchProfileIdRequest {
                launch_profile_id: launch_profile_id.clone()
            }))
            .await
            .unwrap()
        )["agent_instance_id"],
        json!(agent.id)
    );

    let attempt = decoded(
        mcp.launch_enqueue(Parameters(EnqueueLaunchRequest {
            input: input(json!({
                "assignment_id":assignment.id,
                "launch_profile_id":launch_profile_id
            })),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(attempt["status"], "queued");
    let attempt_id = attempt["id"].as_str().unwrap().to_owned();
    assert_eq!(
        decoded(
            mcp.launch_attempt_get(Parameters(LaunchAttemptIdRequest {
                launch_attempt_id: attempt_id
            }))
            .await
            .unwrap()
        )["assignment_id"],
        json!(assignment.id)
    );
    let task_attempts = decoded(
        mcp.task_launch_attempts(Parameters(TaskIdRequest {
            task_id: task.id.to_string(),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(task_attempts.as_array().unwrap().len(), 1);
}
