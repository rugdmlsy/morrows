use morrows_core::{Id, LaunchProfile, Machine};
use morrows_store::Store;

#[derive(Debug, Clone)]
pub enum RuntimeExecutorTarget {
    Local,
    MorrowRuntime {
        machine: Machine,
        worker_name: String,
    },
}

pub async fn resolve(
    store: &Store,
    profile: &LaunchProfile,
    agent_instance_id: Id,
) -> anyhow::Result<RuntimeExecutorTarget> {
    let backend = profile
        .metadata
        .get("execution_backend")
        .and_then(|value| value.as_str())
        .unwrap_or("morrow_runtime");
    match backend {
        "local" => Ok(RuntimeExecutorTarget::Local),
        "morrow_runtime" => {
            let agent = store.get_agent(agent_instance_id).await?;
            let machine_id = agent.machine_id.ok_or_else(|| {
                anyhow::anyhow!("morrow_runtime launch profile requires AgentInstance.machine_id")
            })?;
            let machine = store.get_machine(machine_id).await?;
            let worker_name = machine
                .metadata
                .get("morrow_runtime_worker")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or(machine.name.as_str())
                .to_owned();
            Ok(RuntimeExecutorTarget::MorrowRuntime {
                machine,
                worker_name,
            })
        }
        other => anyhow::bail!("unsupported execution_backend {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use morrows_core::{RegisterAgentInstance, RegisterLaunchProfile};
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};

    fn input<T: DeserializeOwned>(value: Value) -> T {
        serde_json::from_value(value).unwrap()
    }

    #[tokio::test]
    async fn runtime_executor_uses_morrows_machine_as_worker_authority() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let provider = store
            .register_profile(input(json!({"name":"Codex","provider":"openai"})))
            .await
            .unwrap();
        let machine = store
            .register_machine(input(json!({
                "name":"node-01",
                "metadata":{"morrow_runtime_worker":"runtime-node-01"}
            })))
            .await
            .unwrap();
        let agent = store
            .register_agent_instance(input::<RegisterAgentInstance>(json!({
                "name":"codex-remote",
                "profile_id":provider.id,
                "machine_id":machine.id
            })))
            .await
            .unwrap();
        let launch = store
            .register_launch_profile(input::<RegisterLaunchProfile>(json!({
                "name":"remote codex",
                "adapter":"codex_cli",
                "agent_instance_id":agent.id,
                "program":"/opt/codex/bin/codex",
                "default_cwd":"/srv/work"
            })))
            .await
            .unwrap();
        assert_eq!(launch.metadata["execution_backend"], "morrow_runtime");
        match resolve(&store, &launch, agent.id).await.unwrap() {
            RuntimeExecutorTarget::MorrowRuntime {
                machine,
                worker_name,
            } => {
                assert_eq!(machine.name, "node-01");
                assert_eq!(worker_name, "runtime-node-01");
            }
            RuntimeExecutorTarget::Local => panic!("remote profile resolved local"),
        }
    }
}
