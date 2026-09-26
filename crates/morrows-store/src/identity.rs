use super::*;
use morrows_core::{AgentIdentityReport, ReportAgentIdentity};

impl Store {
    pub async fn report_agent_identity(
        &self,
        agent_instance_id: Id,
        input: ReportAgentIdentity,
    ) -> Result<AgentIdentityReport, DomainError> {
        self.get_agent(agent_instance_id).await?;
        let agent_name = normalize_identity_field(input.agent_name, "agent_name", 120)?;
        let account_email = normalize_identity_field(input.account_email, "account_email", 320)?;
        let platform = normalize_identity_field(input.platform, "platform", 80)?;
        let device = normalize_identity_field(input.device, "device", 255)?;
        let id = Uuid::new_v4();
        let reported_at = Utc::now();

        sqlx::query(
            "INSERT INTO agent_identity_reports(
                id,agent_instance_id,agent_name,account_email,platform,device,reported_at
             ) VALUES(?,?,?,?,?,?,?)",
        )
        .bind(id.to_string())
        .bind(agent_instance_id.to_string())
        .bind(agent_name.as_deref())
        .bind(account_email.as_deref())
        .bind(platform.as_deref())
        .bind(device.as_deref())
        .bind(reported_at.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(storage)?;

        Ok(AgentIdentityReport {
            id,
            agent_instance_id,
            agent_name,
            account_email,
            platform,
            device,
            reported_at,
        })
    }

    pub async fn latest_agent_identity_report(
        &self,
        agent_instance_id: Id,
    ) -> Result<Option<AgentIdentityReport>, DomainError> {
        self.get_agent(agent_instance_id).await?;
        let row = sqlx::query(
            "SELECT * FROM agent_identity_reports
             WHERE agent_instance_id=?
             ORDER BY reported_at DESC,id DESC
             LIMIT 1",
        )
        .bind(agent_instance_id.to_string())
        .fetch_optional(&self.pool)
        .await
        .map_err(storage)?;
        row.map(row_to_agent_identity_report).transpose()
    }
}

fn normalize_identity_field(
    value: Option<String>,
    field: &str,
    max_chars: usize,
) -> Result<Option<String>, DomainError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    if value.chars().count() > max_chars {
        return Err(DomainError::InvalidInput(format!(
            "{field} cannot exceed {max_chars} characters"
        )));
    }
    if value.chars().any(char::is_control) {
        return Err(DomainError::InvalidInput(format!(
            "{field} cannot contain control characters"
        )));
    }
    Ok(Some(value.to_owned()))
}

fn row_to_agent_identity_report(
    row: sqlx::sqlite::SqliteRow,
) -> Result<AgentIdentityReport, DomainError> {
    Ok(AgentIdentityReport {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        agent_name: row.try_get("agent_name").map_err(storage)?,
        account_email: row.try_get("account_email").map_err(storage)?,
        platform: row.try_get("platform").map_err(storage)?,
        device: row.try_get("device").map_err(storage)?,
        reported_at: parse_dt(row.try_get("reported_at").map_err(storage)?)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn identity_reports_are_append_only_and_latest_is_returned() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("identity-agent", &[]).await.unwrap();

        let first = store
            .report_agent_identity(
                agent.id,
                ReportAgentIdentity {
                    agent_name: Some("codex-1".into()),
                    account_email: Some("first@example.com".into()),
                    platform: Some("codex".into()),
                    device: Some("node-01".into()),
                },
            )
            .await
            .unwrap();
        let second = store
            .report_agent_identity(
                agent.id,
                ReportAgentIdentity {
                    agent_name: Some("codex-1".into()),
                    account_email: Some("second@example.com".into()),
                    platform: Some("codex".into()),
                    device: Some("node-02".into()),
                },
            )
            .await
            .unwrap();

        assert_ne!(first.id, second.id);
        assert_eq!(
            store.latest_agent_identity_report(agent.id).await.unwrap(),
            Some(second)
        );
    }

    #[tokio::test]
    async fn identity_report_normalizes_blank_unknowns() {
        let store = Store::connect("sqlite::memory:").await.unwrap();
        let agent = store.register_agent("identity-blank", &[]).await.unwrap();
        let report = store
            .report_agent_identity(
                agent.id,
                ReportAgentIdentity {
                    agent_name: Some(" codex-1 ".into()),
                    account_email: Some("  ".into()),
                    platform: Some(" codex ".into()),
                    device: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(report.agent_name.as_deref(), Some("codex-1"));
        assert_eq!(report.account_email, None);
        assert_eq!(report.platform.as_deref(), Some("codex"));
        assert_eq!(report.device, None);
    }
}
