use super::*;
use morrows_core::{AgentCredential, IssuedAgentCredential};
use sha2::{Digest, Sha256};

const TOKEN_PREFIX: &str = "mrw_agent_";
const MAX_BRIDGE_TTL_SECONDS: i64 = 365 * 24 * 60 * 60;
const MAX_RUNTIME_TTL_SECONDS: i64 = 24 * 60 * 60;

impl Store {
    pub async fn issue_bridge_credential(
        &self,
        agent_id: Id,
        label: &str,
        ttl_seconds: i64,
    ) -> Result<IssuedAgentCredential, DomainError> {
        self.issue_agent_credential(
            agent_id,
            "bridge",
            label,
            None,
            None,
            ttl_seconds,
            MAX_BRIDGE_TTL_SECONDS,
        )
        .await
    }

    pub async fn issue_runtime_credential(
        &self,
        agent_id: Id,
        run_id: Id,
        label: &str,
        ttl_seconds: i64,
    ) -> Result<IssuedAgentCredential, DomainError> {
        let run = self.get_run(run_id).await?;
        if run.agent_instance_id != agent_id {
            return Err(DomainError::Conflict(
                "runtime credential Run belongs to another AgentInstance".into(),
            ));
        }
        if !matches!(run.status.as_str(), "running" | "interrupted") {
            return Err(DomainError::Conflict(format!(
                "runtime credential cannot be issued for Run in {}",
                run.status
            )));
        }
        self.issue_agent_credential(
            agent_id,
            "runtime",
            label,
            Some(run_id),
            None,
            ttl_seconds,
            MAX_RUNTIME_TTL_SECONDS,
        )
        .await
    }

    pub async fn issue_session_runtime_credential(
        &self,
        agent_id: Id,
        session_id: Id,
        label: &str,
        ttl_seconds: i64,
    ) -> Result<IssuedAgentCredential, DomainError> {
        let session = self.get_session(session_id).await?;
        if session.agent_instance_id != agent_id {
            return Err(DomainError::Conflict(
                "session runtime credential Session belongs to another AgentInstance".into(),
            ));
        }
        if session.status != "open" {
            return Err(DomainError::Conflict(
                "session runtime credential requires an open Session".into(),
            ));
        }
        self.issue_agent_credential(
            agent_id,
            "session_runtime",
            label,
            None,
            Some(session_id),
            ttl_seconds,
            MAX_RUNTIME_TTL_SECONDS,
        )
        .await
    }

    async fn issue_agent_credential(
        &self,
        agent_id: Id,
        kind: &str,
        label: &str,
        run_id: Option<Id>,
        session_id: Option<Id>,
        ttl_seconds: i64,
        max_ttl_seconds: i64,
    ) -> Result<IssuedAgentCredential, DomainError> {
        self.get_agent(agent_id).await?;
        if !(60..=max_ttl_seconds).contains(&ttl_seconds) {
            return Err(DomainError::InvalidInput(format!(
                "credential ttl_seconds must be between 60 and {max_ttl_seconds}"
            )));
        }
        let label = label.trim();
        if label.is_empty() || label.chars().count() > 160 {
            return Err(DomainError::InvalidInput(
                "credential label must contain 1 to 160 characters".into(),
            ));
        }

        let id = Uuid::new_v4();
        let token = new_agent_token();
        let token_hash = hash_agent_token(&token);
        let now = Utc::now();
        let expires_at = now + Duration::seconds(ttl_seconds);
        sqlx::query(
            "INSERT INTO agent_credentials(
                id,agent_instance_id,token_hash,kind,label,run_id,session_id,expires_at,created_at
             ) VALUES(?,?,?,?,?,?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(agent_id.to_string())
        .bind(token_hash)
        .bind(kind)
        .bind(label)
        .bind(run_id.map(|value| value.to_string()))
        .bind(session_id.map(|value| value.to_string()))
        .bind(expires_at.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(storage)?;

        Ok(IssuedAgentCredential {
            credential: AgentCredential {
                id,
                agent_instance_id: agent_id,
                kind: kind.into(),
                label: label.into(),
                run_id,
                session_id,
                expires_at,
                revoked_at: None,
                created_at: now,
            },
            token,
        })
    }

    pub async fn verify_agent_token(&self, token: &str) -> Result<AgentCredential, DomainError> {
        if !token.starts_with(TOKEN_PREFIX) || token.len() > 256 {
            return Err(DomainError::InvalidInput("invalid Agent credential".into()));
        }
        let token_hash = hash_agent_token(token);
        let row = sqlx::query(
            "SELECT c.* FROM agent_credentials c
             WHERE token_hash=? AND revoked_at IS NULL AND expires_at>?
             AND (c.run_id IS NULL OR EXISTS (
                 SELECT 1 FROM runs r JOIN assignments a ON a.id=r.assignment_id
                 WHERE r.id=c.run_id AND r.agent_instance_id=c.agent_instance_id
                 AND r.status IN ('running','paused') AND a.status='active' AND a.expires_at>?
             ))",
        )
        .bind(token_hash)
        .bind(Utc::now().to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::InvalidInput("invalid or expired Agent credential".into()))?;
        row_to_agent_credential(row)
    }

    pub async fn list_agent_credentials(
        &self,
        agent_id: Id,
    ) -> Result<Vec<AgentCredential>, DomainError> {
        self.get_agent(agent_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM agent_credentials
             WHERE agent_instance_id=? ORDER BY created_at DESC,id DESC",
        )
        .bind(agent_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_agent_credential).collect()
    }

    pub async fn revoke_agent_credential(
        &self,
        credential_id: Id,
    ) -> Result<AgentCredential, DomainError> {
        let now = Utc::now();
        let updated = sqlx::query(
            "UPDATE agent_credentials SET revoked_at=COALESCE(revoked_at,?) WHERE id=?",
        )
        .bind(now.to_rfc3339())
        .bind(credential_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        if updated.rows_affected() == 0 {
            return Err(DomainError::NotFound(format!(
                "agent credential {credential_id}"
            )));
        }
        self.get_agent_credential(credential_id).await
    }

    pub async fn revoke_run_agent_credentials(&self, run_id: Id) -> Result<u64, DomainError> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE agent_credentials SET revoked_at=COALESCE(revoked_at,?)
             WHERE run_id=? AND revoked_at IS NULL",
        )
        .bind(now.to_rfc3339())
        .bind(run_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(result.rows_affected())
    }

    pub async fn revoke_session_agent_credentials(
        &self,
        session_id: Id,
    ) -> Result<u64, DomainError> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE agent_credentials SET revoked_at=COALESCE(revoked_at,?)
             WHERE kind='session_runtime' AND session_id=? AND revoked_at IS NULL",
        )
        .bind(now.to_rfc3339())
        .bind(session_id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        Ok(result.rows_affected())
    }

    pub async fn get_agent_credential(
        &self,
        credential_id: Id,
    ) -> Result<AgentCredential, DomainError> {
        let row = sqlx::query("SELECT * FROM agent_credentials WHERE id=?")
            .bind(credential_id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("agent credential {credential_id}")))?;
        row_to_agent_credential(row)
    }
}

fn new_agent_token() -> String {
    format!(
        "{TOKEN_PREFIX}{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

fn hash_agent_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn row_to_agent_credential(row: sqlx::sqlite::SqliteRow) -> Result<AgentCredential, DomainError> {
    Ok(AgentCredential {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        kind: row.try_get("kind").map_err(storage)?,
        label: row.try_get("label").map_err(storage)?,
        run_id: parse_opt_id(row.try_get("run_id").map_err(storage)?)?,
        session_id: parse_opt_id(row.try_get("session_id").map_err(storage)?)?,
        expires_at: parse_dt(row.try_get("expires_at").map_err(storage)?)?,
        revoked_at: parse_opt_dt(row.try_get("revoked_at").map_err(storage)?)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}
