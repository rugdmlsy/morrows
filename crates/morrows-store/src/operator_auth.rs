use super::*;
use morrows_core::{IssuedOperatorCredential, OperatorCredential, OperatorLoginRequest};
use sha2::{Digest, Sha256};

const TOKEN_PREFIX: &str = "mrw_operator_";
const MAX_TTL_SECONDS: i64 = 365 * 24 * 60 * 60;

impl Store {
    /// The public request grants no authority. Only the service owner's local
    /// approval can activate its secret, and the database never stores that secret.
    pub async fn request_operator_login(
        &self,
        label: &str,
    ) -> Result<OperatorLoginRequest, DomainError> {
        let label = label.trim();
        if label.is_empty() || label.chars().count() > 160 {
            return Err(DomainError::InvalidInput(
                "login label must contain 1 to 160 characters".into(),
            ));
        }
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM operator_login_requests WHERE credential_id IS NULL AND expires_at>?")
            .bind(now.to_rfc3339()).fetch_one(&mut *tx).await.map_err(storage)?;
        if pending >= 64 {
            return Err(DomainError::Conflict(
                "too many pending logins; retry after ten minutes".into(),
            ));
        }
        let id = Uuid::new_v4();
        let code = Uuid::new_v4().simple().to_string()[..12].to_uppercase();
        let token = new_operator_token();
        let expires_at = now + Duration::minutes(10);
        sqlx::query("INSERT INTO operator_login_requests(id,code,token_hash,label,created_at,expires_at) VALUES(?,?,?,?,?,?)")
            .bind(id.to_string()).bind(&code).bind(hash_operator_token(&token)).bind(label)
            .bind(now.to_rfc3339()).bind(expires_at.to_rfc3339())
            .execute(&mut *tx).await.map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        Ok(OperatorLoginRequest {
            id,
            code,
            token,
            expires_at,
        })
    }

    pub async fn operator_login_status(&self, id: Id, token: &str) -> Result<Value, DomainError> {
        let row = sqlx::query("SELECT credential_id,expires_at FROM operator_login_requests WHERE id=? AND token_hash=?")
            .bind(id.to_string()).bind(hash_operator_token(token))
            .fetch_optional(&self.pool).await.map_err(storage)?
            .ok_or_else(|| DomainError::InvalidInput("invalid login request".into()))?;
        if let Some(raw) = row
            .try_get::<Option<String>, _>("credential_id")
            .map_err(storage)?
        {
            let credential = self.get_operator_credential(parse_id(raw)?).await?;
            if credential.revoked_at.is_some() || credential.expires_at <= Utc::now() {
                return Ok(json!({"status":"expired"}));
            }
            return Ok(json!({"status":"approved", "credential":credential}));
        }
        let expiry = parse_dt(row.try_get("expires_at").map_err(storage)?)?;
        Ok(json!({"status":if expiry > Utc::now() {"pending"} else {"expired"}}))
    }

    /// Runs only in the local administrative CLI, never on a public HTTP route.
    /// Promotion is atomic and idempotent: a retry returns the same credential
    /// and cannot extend its lifetime or increase its original role.
    pub async fn approve_operator_login(
        &self,
        code: &str,
        role: &str,
        ttl_seconds: i64,
    ) -> Result<OperatorCredential, DomainError> {
        if !matches!(role, "viewer" | "operator" | "admin")
            || !(60..=MAX_TTL_SECONDS).contains(&ttl_seconds)
        {
            return Err(DomainError::InvalidInput(
                "invalid login role or lifetime".into(),
            ));
        }
        let code = code.trim().to_ascii_uppercase();
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row = sqlx::query("SELECT * FROM operator_login_requests WHERE code=?")
            .bind(&code)
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound("login code".into()))?;
        if let Some(raw) = row
            .try_get::<Option<String>, _>("credential_id")
            .map_err(storage)?
        {
            tx.commit().await.map_err(storage)?;
            return self.get_operator_credential(parse_id(raw)?).await;
        }
        if parse_dt(row.try_get("expires_at").map_err(storage)?)? <= now {
            return Err(DomainError::Conflict(
                "login code expired; start again in the browser".into(),
            ));
        }
        let credential = OperatorCredential {
            id: Uuid::new_v4(),
            label: format!(
                "SSH browser login: {}",
                row.try_get::<String, _>("label").map_err(storage)?
            ),
            role: role.into(),
            expires_at: now + Duration::seconds(ttl_seconds),
            revoked_at: None,
            created_at: now,
        };
        sqlx::query("INSERT INTO operator_credentials(id,token_hash,label,role,expires_at,created_at) VALUES(?,?,?,?,?,?)")
            .bind(credential.id.to_string()).bind(row.try_get::<String,_>("token_hash").map_err(storage)?)
            .bind(&credential.label).bind(role).bind(credential.expires_at.to_rfc3339()).bind(now.to_rfc3339())
            .execute(&mut *tx).await.map_err(storage)?;
        sqlx::query("UPDATE operator_login_requests SET credential_id=? WHERE code=? AND credential_id IS NULL")
            .bind(credential.id.to_string()).bind(code).execute(&mut *tx).await.map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        Ok(credential)
    }

    pub async fn issue_operator_credential(
        &self,
        label: &str,
        role: &str,
        ttl_seconds: i64,
    ) -> Result<IssuedOperatorCredential, DomainError> {
        let role = role.trim();
        if !matches!(role, "viewer" | "operator" | "admin") {
            return Err(DomainError::InvalidInput(
                "operator credential role must be viewer, operator, or admin".into(),
            ));
        }
        if !(60..=MAX_TTL_SECONDS).contains(&ttl_seconds) {
            return Err(DomainError::InvalidInput(format!(
                "operator credential ttl_seconds must be between 60 and {MAX_TTL_SECONDS}"
            )));
        }
        let label = label.trim();
        if label.is_empty() || label.chars().count() > 160 {
            return Err(DomainError::InvalidInput(
                "operator credential label must contain 1 to 160 characters".into(),
            ));
        }

        let id = Uuid::new_v4();
        let token = new_operator_token();
        let now = Utc::now();
        let expires_at = now + Duration::seconds(ttl_seconds);
        sqlx::query(
            "INSERT INTO operator_credentials(
                id,token_hash,label,role,expires_at,created_at
             ) VALUES(?,?,?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(hash_operator_token(&token))
        .bind(label)
        .bind(role)
        .bind(expires_at.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(storage)?;

        Ok(IssuedOperatorCredential {
            credential: OperatorCredential {
                id,
                label: label.into(),
                role: role.into(),
                expires_at,
                revoked_at: None,
                created_at: now,
            },
            token,
        })
    }

    pub async fn verify_operator_token(
        &self,
        token: &str,
    ) -> Result<OperatorCredential, DomainError> {
        if !token.starts_with(TOKEN_PREFIX) || token.len() > 256 {
            return Err(DomainError::InvalidInput(
                "invalid operator credential".into(),
            ));
        }
        let row = sqlx::query(
            "SELECT * FROM operator_credentials
             WHERE token_hash=? AND revoked_at IS NULL AND expires_at>?",
        )
        .bind(hash_operator_token(token))
        .bind(Utc::now().to_rfc3339())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?
        .ok_or_else(|| {
            DomainError::InvalidInput("invalid or expired operator credential".into())
        })?;
        row_to_operator_credential(row)
    }

    pub async fn list_operator_credentials(&self) -> Result<Vec<OperatorCredential>, DomainError> {
        let rows =
            sqlx::query("SELECT * FROM operator_credentials ORDER BY created_at DESC,id DESC")
                .fetch_all(&self.pool)
                .await
                .map_err(storage)?;
        rows.into_iter().map(row_to_operator_credential).collect()
    }

    pub async fn revoke_operator_credential(
        &self,
        id: Id,
    ) -> Result<OperatorCredential, DomainError> {
        let now = Utc::now();
        let result = sqlx::query(
            "UPDATE operator_credentials
             SET revoked_at=COALESCE(revoked_at,?)
             WHERE id=?",
        )
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        if result.rows_affected() == 0 {
            return Err(DomainError::NotFound(format!("operator credential {id}")));
        }
        self.get_operator_credential(id).await
    }

    pub async fn get_operator_credential(&self, id: Id) -> Result<OperatorCredential, DomainError> {
        let row = sqlx::query("SELECT * FROM operator_credentials WHERE id=?")
            .bind(id.to_string())
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("operator credential {id}")))?;
        row_to_operator_credential(row)
    }

    pub async fn has_active_admin_operator_credential(&self) -> Result<bool, DomainError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM operator_credentials
             WHERE role='admin' AND revoked_at IS NULL AND expires_at>?",
        )
        .bind(Utc::now().to_rfc3339())
        .fetch_one(&self.pool)
        .await
        .map_err(storage)?;
        Ok(count > 0)
    }
}

fn new_operator_token() -> String {
    format!(
        "{TOKEN_PREFIX}{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

fn hash_operator_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut output, "{byte:02x}");
    }
    output
}

fn row_to_operator_credential(
    row: sqlx::sqlite::SqliteRow,
) -> Result<OperatorCredential, DomainError> {
    Ok(OperatorCredential {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        label: row.try_get("label").map_err(storage)?,
        role: row.try_get("role").map_err(storage)?,
        expires_at: parse_dt(row.try_get("expires_at").map_err(storage)?)?,
        revoked_at: parse_opt_dt(row.try_get("revoked_at").map_err(storage)?)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}
