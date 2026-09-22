use super::*;

fn parts(actor: Option<Id>) -> Parts {
    let mut req = axum::http::Request::builder();
    if let Some(id) = actor {
        req = req.header("x-agent-instance-id", id.to_string());
    }
    req.body(()).unwrap().into_parts().0
}
fn input<T: serde::de::DeserializeOwned>(v: Value) -> T {
    serde_json::from_value(v).unwrap()
}
fn decoded(s: String) -> Value {
    serde_json::from_str(&s).unwrap()
}

#[tokio::test]
async fn mcp_fleet_registration_and_owned_observations() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let other = store.register_agent("other", &[]).await.unwrap();
    let mcp = AgentCompanyMcp::new(store.clone());
    for name in [
        "agent_profile_register",
        "agent_profile_list",
        "agent_profile_get",
        "account_register",
        "account_list",
        "account_get",
        "machine_register",
        "machine_list",
        "machine_get",
        "agent_instance_register",
        "agent_heartbeat",
        "capacity_record",
        "capacity_latest",
        "capacity_history",
        "agent_fleet",
    ] {
        assert!(mcp.tool_router.get(name).is_some(), "missing tool {name}");
    }
    let p = decoded(
        mcp.agent_profile_register(Parameters(input(
            json!({"name":"Codex","provider":"openai","default_capabilities":["code"]}),
        )))
        .await
        .unwrap(),
    );
    let a = decoded(
        mcp.account_register(Parameters(input(
            json!({"provider":"openai","label":"personal"}),
        )))
        .await
        .unwrap(),
    );
    let m = decoded(
        mcp.machine_register(Parameters(input(json!({"name":"mac"}))))
            .await
            .unwrap(),
    );
    let get_req = |value: &Value| {
        Parameters(FleetIdRequest {
            id: value["id"].as_str().unwrap().into(),
        })
    };
    assert_eq!(
        decoded(mcp.agent_profile_get(get_req(&p)).await.unwrap()),
        p
    );
    assert_eq!(decoded(mcp.account_get(get_req(&a)).await.unwrap()), a);
    assert_eq!(decoded(mcp.machine_get(get_req(&m)).await.unwrap()), m);
    assert!(
        decoded(mcp.agent_profile_list().await.unwrap())
            .as_array()
            .unwrap()
            .contains(&p)
    );
    assert!(
        decoded(mcp.account_list().await.unwrap())
            .as_array()
            .unwrap()
            .contains(&a)
    );
    assert!(
        decoded(mcp.machine_list().await.unwrap())
            .as_array()
            .unwrap()
            .contains(&m)
    );
    let worker = decoded(
        mcp.agent_instance_register(Parameters(input(
            json!({"name":"worker","profile_id":p["id"],"account_id":a["id"],"machine_id":m["id"]}),
        )))
        .await
        .unwrap(),
    );
    let id: Id = input(worker["id"].clone());
    assert_eq!(worker["capabilities"], json!(["code"]));
    assert_eq!(worker["account_id"], a["id"]);
    let cap = json!({"status":"ready","available_slots":1,"active_assignments":0,"active_runs":0,"max_concurrency":1});
    let heartbeat = || {
        Parameters(HeartbeatRequest {
            agent_instance_id: id.to_string(),
            input: input(json!({"status":"busy","capacity":cap})),
        })
    };
    let capacity = || {
        Parameters(CapacityRequest {
            agent_instance_id: id.to_string(),
            input: input(cap.clone()),
        })
    };
    for actor in [None, Some(other.id)] {
        assert!(
            mcp.agent_heartbeat(heartbeat(), Extension(parts(actor)))
                .await
                .is_err()
        );
        assert!(
            mcp.capacity_record(capacity(), Extension(parts(actor)))
                .await
                .is_err()
        );
    }
    mcp.agent_heartbeat(heartbeat(), Extension(parts(Some(id))))
        .await
        .unwrap();
    let recorded = decoded(
        mcp.capacity_record(capacity(), Extension(parts(Some(id))))
            .await
            .unwrap(),
    );
    let latest = decoded(
        mcp.capacity_latest(Parameters(InstanceIdRequest {
            agent_instance_id: id.to_string(),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(recorded, latest);
    let history = decoded(
        mcp.capacity_history(Parameters(InstanceIdRequest {
            agent_instance_id: id.to_string(),
        }))
        .await
        .unwrap(),
    );
    assert_eq!(history.as_array().unwrap().len(), 2);
    let fleet = decoded(mcp.agent_fleet().await.unwrap());
    let entry = fleet
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["instance"]["id"] == worker["id"])
        .unwrap();
    assert_eq!(entry["profile"]["id"], p["id"]);
    assert_eq!(entry["latest_capacity"], latest);
    assert!(entry["machine"]["last_seen_at"].is_string());
    let mut invalid = capacity();
    invalid.0.input.available_slots = 2;
    assert!(
        mcp.capacity_record(invalid, Extension(parts(Some(id))))
            .await
            .is_err()
    );
    assert!(
        mcp.capacity_history(Parameters(InstanceIdRequest {
            agent_instance_id: "bad-id".into()
        }))
        .await
        .is_err()
    );
    let refreshed = decoded(
        mcp.agent_register(Parameters(RegisterAgentRequest {
            name: "worker".into(),
            capabilities: vec!["test".into()],
        }))
        .await
        .unwrap(),
    );
    assert_eq!(refreshed["id"], worker["id"]);
    assert_eq!(refreshed["profile_id"], p["id"]);
}
