use anyhow::{Context, anyhow};
use morrows_core::Id;
use morrows_store::Store;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct LsmControl {
    origin: String,
    address: std::net::SocketAddr,
    key: String,
    subject: String,
}

pub struct AgentBinding {
    pub logical_session_id: String,
    pub capability: String,
    pub mcp_url: String,
}

impl LsmControl {
    pub fn subject(&self) -> &str {
        &self.subject
    }

    pub fn from_env() -> anyhow::Result<Option<Self>> {
        let Ok(configured) = std::env::var("MORROWS_LSM_CONTROL_URL") else {
            return Ok(None);
        };
        let origin = configured
            .trim_end_matches('/')
            .trim_end_matches("/api/control")
            .to_owned();
        let authority = origin
            .strip_prefix("http://")
            .ok_or_else(|| anyhow!("MORROWS_LSM_CONTROL_URL must use loopback HTTP"))?;
        let address: std::net::SocketAddr = authority
            .parse()
            .context("MORROWS_LSM_CONTROL_URL must contain a numeric host and port")?;
        if !address.ip().is_loopback() {
            anyhow::bail!("MORROWS_LSM_CONTROL_URL must target loopback");
        }
        let key = std::env::var("MORROWS_LSM_CONTROL_KEY")
            .context("MORROWS_LSM_CONTROL_KEY is required when LSM integration is enabled")?;
        if key.is_empty() || key.contains('\r') || key.contains('\n') {
            anyhow::bail!("MORROWS_LSM_CONTROL_KEY is empty or invalid");
        }
        Ok(Some(Self {
            origin,
            address,
            key,
            subject: std::env::var("MORROWS_LSM_SUBJECT")
                .unwrap_or_else(|_| "local-mcp-client".into()),
        }))
    }

    pub async fn request(&self, method: &str, path: &str, body: Value) -> anyhow::Result<Value> {
        let payload = if method == "GET" {
            Vec::new()
        } else {
            serde_json::to_vec(&body)?
        };
        let mut stream = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio::net::TcpStream::connect(self.address),
        )
        .await??;
        let request = format!(
            "{method} /api/control{path} HTTP/1.1\r\nHost: {}\r\nX-LSM-Control-Key: {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            self.address,
            self.key,
            payload.len(),
        );
        stream.write_all(request.as_bytes()).await?;
        stream.write_all(&payload).await?;
        let mut response = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(40),
            stream.read_to_end(&mut response),
        )
        .await??;
        let separator = response
            .windows(4)
            .position(|chunk| chunk == b"\r\n\r\n")
            .ok_or_else(|| anyhow!("LSM control response has no headers"))?;
        let header = std::str::from_utf8(&response[..separator])?;
        let status = header
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u16>().ok())
            .ok_or_else(|| anyhow!("LSM control response has no status"))?;
        let value: Value = serde_json::from_slice(&response[separator + 4..])
            .context("LSM control response is not JSON")?;
        if !(200..300).contains(&status) {
            anyhow::bail!("LSM control returned {status}: {}", value["error"]);
        }
        Ok(value)
    }

    /// Replay the Run key before issuing any Agent credential. A lost HTTP
    /// response or failed Morrows binding write can therefore be reconciled.
    pub async fn provision_run(
        &self,
        store: &Store,
        run_id: Id,
        task_title: &str,
    ) -> anyhow::Result<String> {
        let session_id = if let Some(existing) = store.run_lsm_binding(run_id).await? {
            existing.logical_session_id
        } else {
            let subject = store
                .run_lsm_provisioning_subject(run_id)
                .await?
                .unwrap_or_else(|| self.subject.clone());
            let response = self
                .request(
                    "POST",
                    "/sessions",
                    json!({
                        "subject": subject,
                        "idempotency_key": format!("morrows:run:{run_id}"),
                        "label": format!("Morrows: {task_title}"),
                        "objective": task_title,
                    }),
                )
                .await?;
            let session_id = response["session"]["session_id"]
                .as_str()
                .ok_or_else(|| anyhow!("LSM did not return a Session ID"))?
                .to_owned();
            store.bind_run_lsm(run_id, &session_id).await?;
            session_id
        };
        Ok(session_id)
    }

    pub async fn provision_agent(
        &self,
        store: &Store,
        run_id: Id,
        task_title: &str,
    ) -> anyhow::Result<AgentBinding> {
        let session_id = self.provision_run(store, run_id, task_title).await?;
        let subject = store
            .run_lsm_provisioning_subject(run_id)
            .await?
            .unwrap_or_else(|| self.subject.clone());
        let issued = self
            .request(
                "POST",
                &format!("/sessions/{session_id}/capabilities"),
                json!({"subject":subject}),
            )
            .await?;
        let capability = issued["capability"]
            .as_str()
            .ok_or_else(|| anyhow!("LSM did not return a capability"))?
            .to_owned();
        let capability_id = issued["capability_id"]
            .as_str()
            .ok_or_else(|| anyhow!("LSM did not return a capability ID"))?;
        if let Err(err) = store.try_adopt_run_capability(run_id, capability_id).await {
            // The Run state write wins when cancellation races with issuance.
            // The unadopted credential must be revoked before aborting launch.
            if let Err(revoke_err) = self
                .request(
                    "POST",
                    &format!("/capabilities/{capability_id}/revoke"),
                    json!({}),
                )
                .await
            {
                tracing::warn!(%run_id, %capability_id, %revoke_err,
                    "failed revoking unadopted LSM capability; runtime cleanup will retry");
            }
            return Err(err.into());
        }
        Ok(AgentBinding {
            logical_session_id: session_id,
            capability,
            mcp_url: format!("{}/mcp", self.origin),
        })
    }

    pub async fn revoke_for_run(&self, store: &Store, run_id: Id) -> anyhow::Result<()> {
        if let Some(binding) = store.run_lsm_binding(run_id).await?
            && let Some(capability_id) = binding.capability_id
        {
            self.request(
                "POST",
                &format!("/capabilities/{capability_id}/revoke"),
                json!({}),
            )
            .await?;
            store
                .clear_run_capability_if_matches(run_id, &capability_id)
                .await?;
        }
        Ok(())
    }

    pub async fn cleanup_run(
        &self,
        store: &Store,
        run_id: Id,
        session_id: &str,
        finish: bool,
    ) -> anyhow::Result<bool> {
        let subject = store
            .run_lsm_provisioning_subject(run_id)
            .await?
            .unwrap_or_else(|| self.subject.clone());
        let response = self
            .request(
                "POST",
                &format!("/sessions/{session_id}/cleanup"),
                json!({"subject":subject,"wait_seconds":30,
                   "terminal_action":if finish {"finish"} else {"cancel"}}),
            )
            .await?;
        Ok(response["complete"].as_bool().unwrap_or(false))
    }

    pub async fn observe(&self, session_id: &str) -> anyhow::Result<Value> {
        let session = self
            .request("GET", &format!("/sessions/{session_id}"), json!({}))
            .await?;
        let jobs = self
            .request("GET", &format!("/sessions/{session_id}/jobs"), json!({}))
            .await?;
        let shells = self
            .request("GET", &format!("/sessions/{session_id}/shells"), json!({}))
            .await?;
        let audit = self
            .request(
                "GET",
                &format!("/sessions/{session_id}/audit?limit=200"),
                json!({}),
            )
            .await?;
        Ok(json!({"session":session["session"],"jobs":jobs,"shells":shells,"audit":audit}))
    }

    pub async fn job_tail(
        &self,
        session_id: &str,
        job_id: &str,
        machine: &str,
    ) -> anyhow::Result<Value> {
        let machine = machine
            .as_bytes()
            .iter()
            .map(|byte| {
                if byte.is_ascii_alphanumeric() || b"-._~".contains(byte) {
                    (*byte as char).to_string()
                } else {
                    format!("%{byte:02X}")
                }
            })
            .collect::<String>();
        self.request(
            "GET",
            &format!("/sessions/{session_id}/jobs/{job_id}/tail?machine={machine}&lines=200"),
            json!({}),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, extract::State, routing::post};
    use std::sync::Arc;
    use tokio::sync::{Mutex, oneshot};

    struct MockCapability {
        active: bool,
        revocations: usize,
        issued: Option<oneshot::Sender<()>>,
        release: Option<oneshot::Receiver<()>>,
    }

    async fn issue(State(state): State<Arc<Mutex<MockCapability>>>) -> Json<Value> {
        let release = {
            let mut state = state.lock().await;
            state.active = true;
            state.issued.take().unwrap().send(()).unwrap();
            state.release.take().unwrap()
        };
        release.await.unwrap();
        Json(json!({"capability":"token-issued-during-cancel", "capability_id":"cap-race"}))
    }

    async fn revoke(State(state): State<Arc<Mutex<MockCapability>>>) -> Json<Value> {
        let mut state = state.lock().await;
        state.active = false;
        state.revocations += 1;
        Json(json!({"revoked":true}))
    }

    #[tokio::test]
    async fn capability_issued_after_cancel_is_revoked_before_launch() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store
            .register_agent("race-agent".into(), &[])
            .await
            .unwrap();
        let task = store
            .create_task(serde_json::from_value(json!({"title":"issue cancel race"})).unwrap())
            .await
            .unwrap();
        let assignment = store
            .claim_task(task.id, agent.id, "executor", 600)
            .await
            .unwrap();
        let profile = store
            .register_launch_profile(
                serde_json::from_value(json!({
                    "name":"race-codex", "adapter":"codex_cli", "agent_instance_id":agent.id,
                    "program":"/bin/echo", "default_cwd":"/tmp"
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        let attempt = store
            .enqueue_launch(
                serde_json::from_value(json!({
                    "assignment_id":assignment.id, "launch_profile_id":profile.id
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        store.claim_launch_job().await.unwrap().unwrap();
        let run_id = store
            .begin_launch_attempt_with_lsm(attempt.id, Some("shared-runtime"))
            .await
            .unwrap()
            .run
            .id;
        store
            .bind_run_lsm(run_id, "s_issue_cancel_race")
            .await
            .unwrap();

        let (issued_tx, issued_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        let state = Arc::new(Mutex::new(MockCapability {
            active: false,
            revocations: 0,
            issued: Some(issued_tx),
            release: Some(release_rx),
        }));
        let app = Router::new()
            .route("/api/control/sessions/{id}/capabilities", post(issue))
            .route("/api/control/capabilities/{id}/revoke", post(revoke))
            .with_state(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let control = LsmControl {
            origin: format!("http://{address}"),
            address,
            key: "test-control-key".into(),
            subject: "shared-runtime".into(),
        };
        let launcher_control = control.clone();
        let launcher_store = store.clone();
        let launcher = tokio::spawn(async move {
            launcher_control
                .provision_agent(&launcher_store, run_id, "issue cancel race")
                .await
        });

        tokio::time::timeout(std::time::Duration::from_secs(5), issued_rx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            store.request_run_cancel(run_id).await.unwrap().status,
            "cancelling"
        );
        control.revoke_for_run(&store, run_id).await.unwrap();
        assert!(state.lock().await.active);
        release_tx.send(()).unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(5), launcher)
                .await
                .unwrap()
                .unwrap()
                .is_err()
        );
        let state = state.lock().await;
        assert!(!state.active);
        assert_eq!(state.revocations, 1);
        assert!(
            store
                .run_lsm_binding(run_id)
                .await
                .unwrap()
                .unwrap()
                .capability_id
                .is_none()
        );
        assert!(
            store
                .mark_launch_running(attempt.id, Some(42), "out".into(), "err".into())
                .await
                .is_err()
        );
        assert_eq!(
            store.get_launch_attempt(attempt.id).await.unwrap().status,
            "starting"
        );
        server.abort();
    }
}
