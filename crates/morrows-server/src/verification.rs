use crate::morrow_runtime::MorrowRuntimeControl;
use morrows_core::{DomainError, Id, VerificationMode};
use morrows_store::Store;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ReviewContextRequest {
    pub run_id: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct VerifyRequest {
    pub run_id: String,
    pub criterion_id: String,
    pub retry_reason: Option<String>,
}
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ReviewRequest {
    pub reviewer_run_id: String,
    pub executor_run_id: String,
    pub criterion_id: String,
    pub verdict: String,
    /// Explain the evidence and any correction needed.
    pub rationale: String,
    pub artifact_ids: Vec<String>,
    #[schemars(with = "Option<String>")]
    pub context_revision_id: Option<Id>,
    pub acceptance_version: i64,
}

pub async fn verify(
    store: &Store,
    control: &MorrowRuntimeControl,
    agent: Id,
    req: VerifyRequest,
) -> Result<Value, DomainError> {
    let run_id = Id::parse_str(&req.run_id)
        .map_err(|_| DomainError::InvalidInput("invalid run_id".into()))?;
    let (_, task) = store.authorize_run_runtime_access(run_id, agent).await?;
    // Provision first: configuration failures do not leave a check reservation.
    // All command/path/machine values come from the immutable Task criterion.
    let scope = control
        .provision_run(store, run_id, &task.title)
        .await
        .map_err(runtime_error)?;
    let (id, c) = store
        .begin_verification(
            run_id,
            agent,
            &req.criterion_id,
            req.retry_reason.as_deref(),
        )
        .await?;
    let (tool, args) = match c.verification {
        VerificationMode::DeterministicCheck => {
            let check = c.check.expect("validated criterion has check");
            (
                "run_shell",
                json!({"command":check.command,"cwd":check.cwd,"machine":check.machine,"timeout_s":check.timeout_s,"max_output_bytes":8000,"purpose":"Task acceptance verification"}),
            )
        }
        VerificationMode::ArtifactCheck => {
            let check = c
                .artifact_check
                .expect("validated criterion has artifact check");
            (
                "file_read",
                json!({"path":check.path,"machine":check.machine,"start_line":1,"end_line":100,"binary_preview":"hex","binary_preview_bytes":128}),
            )
        }
        _ => unreachable!("begin_verification limits verification modes"),
    };
    // The existing Run-owned runtime path enforces the same worker/scope policy
    // as ordinary execution; verification never gets a new privileged scope.
    let outcome = control
        .call_run_tool(store, run_id, &scope, tool, args.clone())
        .await;
    let (verdict, detail) = match outcome {
        Ok(raw) => {
            let result = &raw["result"];
            let failed = raw.get("error").is_some()
                || result.get("error").is_some()
                || result["ok"] == false
                || result["status"] == "error"
                || result["timed_out"] == true;
            let passed = !failed
                && if tool == "run_shell" {
                    result["exit_code"] == 0 && result["ok"] == true
                } else {
                    result.is_object() && !result.as_object().is_none_or(|o| o.is_empty())
                };
            (
                if passed { "PASS" } else { "FAIL" },
                json!({"command_or_path":args,"runtime_scope_id":scope,"runtime":raw}),
            )
        }
        Err(error) => (
            "BLOCKED",
            json!({"command_or_path":args,"runtime_scope_id":scope,"error":error.to_string(),"reconciliation_required":true}),
        ),
    };
    store.finish_verification(&id, verdict, detail).await
}
fn runtime_error(error: anyhow::Error) -> DomainError {
    DomainError::Conflict(format!("runtime verification unavailable: {error}"))
}
