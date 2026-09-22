use super::*;
use morrows_core::*;

impl Store {
    /// Register by provider + name; an existing identity is returned unchanged.
    pub async fn register_profile(
        &self,
        input: RegisterProfile,
    ) -> Result<AgentProfile, DomainError> {
        nonempty(&input.name, "name")?;
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO agent_profiles(id,name,provider,kind,default_capabilities_json,metadata_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(provider,name) DO NOTHING")
            .bind(Uuid::new_v4().to_string()).bind(&input.name).bind(&input.provider).bind(&input.kind).bind(serde_json::to_string(&input.default_capabilities).map_err(storage)?).bind(serde_json::to_string(&input.metadata).map_err(storage)?)
            .bind(&now).bind(&now).execute(&self.pool).await.map_err(storage)?;
        let row = sqlx::query("SELECT * FROM agent_profiles WHERE provider=? AND name=?")
            .bind(&input.provider)
            .bind(&input.name)
            .fetch_one(&self.pool)
            .await
            .map_err(storage)?;
        row_to_profile(row)
    }
    pub async fn get_profile(&self, id: Id) -> Result<AgentProfile, DomainError> {
        row_to_profile(
            sqlx::query("SELECT * FROM agent_profiles WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("profile {id}")))?,
        )
    }
    pub async fn list_profiles(&self) -> Result<Vec<AgentProfile>, DomainError> {
        sqlx::query("SELECT * FROM agent_profiles ORDER BY provider,name,id")
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
            .into_iter()
            .map(row_to_profile)
            .collect()
    }
    /// Register by provider + label; an existing identity is returned unchanged.
    pub async fn register_account(&self, input: RegisterAccount) -> Result<Account, DomainError> {
        nonempty(&input.label, "label")?;
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO accounts(id,provider,label,external_account_ref,status,metadata_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(provider,label) DO NOTHING")
            .bind(Uuid::new_v4().to_string()).bind(&input.provider).bind(&input.label).bind(&input.external_account_ref).bind(&input.status).bind(serde_json::to_string(&input.metadata).map_err(storage)?)
            .bind(&now).bind(&now).execute(&self.pool).await.map_err(storage)?;
        let row = sqlx::query("SELECT * FROM accounts WHERE provider=? AND label=?")
            .bind(&input.provider)
            .bind(&input.label)
            .fetch_one(&self.pool)
            .await
            .map_err(storage)?;
        row_to_account(row)
    }
    pub async fn get_account(&self, id: Id) -> Result<Account, DomainError> {
        row_to_account(
            sqlx::query("SELECT * FROM accounts WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("account {id}")))?,
        )
    }
    pub async fn list_accounts(&self) -> Result<Vec<Account>, DomainError> {
        sqlx::query("SELECT * FROM accounts ORDER BY provider,label,id")
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
            .into_iter()
            .map(row_to_account)
            .collect()
    }
    /// Register by name; an existing identity is returned unchanged.
    pub async fn register_machine(&self, input: RegisterMachine) -> Result<Machine, DomainError> {
        nonempty(&input.name, "name")?;
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO machines(id,name,hostname,os,arch,status,metadata_json,created_at,updated_at) VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(name) DO NOTHING")
            .bind(Uuid::new_v4().to_string()).bind(&input.name).bind(&input.hostname).bind(&input.os).bind(&input.arch).bind(&input.status).bind(serde_json::to_string(&input.metadata).map_err(storage)?)
            .bind(&now).bind(&now).execute(&self.pool).await.map_err(storage)?;
        let row = sqlx::query("SELECT * FROM machines WHERE name=?")
            .bind(&input.name)
            .fetch_one(&self.pool)
            .await
            .map_err(storage)?;
        row_to_machine(row)
    }
    pub async fn get_machine(&self, id: Id) -> Result<Machine, DomainError> {
        row_to_machine(
            sqlx::query("SELECT * FROM machines WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("machine {id}")))?,
        )
    }
    pub async fn list_machines(&self) -> Result<Vec<Machine>, DomainError> {
        sqlx::query("SELECT * FROM machines ORDER BY name,id")
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
            .into_iter()
            .map(row_to_machine)
            .collect()
    }
}

fn row_to_profile(row: sqlx::sqlite::SqliteRow) -> Result<AgentProfile, DomainError> {
    Ok(AgentProfile {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        name: row.try_get("name").map_err(storage)?,
        provider: row.try_get("provider").map_err(storage)?,
        kind: row.try_get("kind").map_err(storage)?,
        default_capabilities: serde_json::from_str(
            &row.try_get::<String, _>("default_capabilities_json")
                .map_err(storage)?,
        )
        .map_err(storage)?,
        metadata: serde_json::from_str(
            &row.try_get::<String, _>("metadata_json").map_err(storage)?,
        )
        .map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}
fn row_to_account(row: sqlx::sqlite::SqliteRow) -> Result<Account, DomainError> {
    Ok(Account {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        provider: row.try_get("provider").map_err(storage)?,
        label: row.try_get("label").map_err(storage)?,
        external_account_ref: row.try_get("external_account_ref").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        metadata: serde_json::from_str(
            &row.try_get::<String, _>("metadata_json").map_err(storage)?,
        )
        .map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}
fn row_to_machine(row: sqlx::sqlite::SqliteRow) -> Result<Machine, DomainError> {
    Ok(Machine {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        name: row.try_get("name").map_err(storage)?,
        hostname: row.try_get("hostname").map_err(storage)?,
        os: row.try_get("os").map_err(storage)?,
        arch: row.try_get("arch").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        metadata: serde_json::from_str(
            &row.try_get::<String, _>("metadata_json").map_err(storage)?,
        )
        .map_err(storage)?,
        last_seen_at: parse_opt_dt(row.try_get("last_seen_at").map_err(storage)?)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}

const LEGACY_PROFILE: &str = "00000000-0000-0000-0000-000000000003";

fn nonempty(value: &str, field: &str) -> Result<(), DomainError> {
    if value.trim().is_empty() {
        return Err(DomainError::InvalidInput(format!(
            "{field} cannot be empty"
        )));
    }
    Ok(())
}

impl Store {
    /// Serialize name lookup and insertion so concurrent legacy calls reuse the
    /// same UUID. Existing normalized links are never overwritten by a refresh.
    pub async fn register_agent(
        &self,
        name: &str,
        capabilities: &[String],
    ) -> Result<AgentInstance, DomainError> {
        nonempty(name, "agent name")?;
        let name = name.trim();
        let now = Utc::now().to_rfc3339();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let existing: Option<String> =
            sqlx::query_scalar("SELECT id FROM agent_instances WHERE name=?")
                .bind(name)
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?;
        let id = if let Some(id) = existing {
            sqlx::query("UPDATE agent_instances SET status='online',capabilities_json=?,last_heartbeat_at=? WHERE id=?")
                .bind(json!(capabilities).to_string()).bind(&now).bind(&id).execute(&mut *tx).await.map_err(storage)?;
            parse_id(id)?
        } else {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO accounts(id,provider,label,status,metadata_json,created_at,updated_at) VALUES(?,'legacy',?,'unknown','{\"legacy\":true}',?,?) ON CONFLICT(provider,label) DO NOTHING")
                .bind(id.to_string()).bind(name).bind(&now).bind(&now).execute(&mut *tx).await.map_err(storage)?;
            let account_id: String =
                sqlx::query_scalar("SELECT id FROM accounts WHERE provider='legacy' AND label=?")
                    .bind(name)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage)?;
            sqlx::query("INSERT INTO machines(id,name,hostname,os,arch,status,metadata_json,last_seen_at,created_at,updated_at) VALUES(?,?,'unknown','unknown','unknown','unknown','{\"legacy\":true}',?,?,?)")
                .bind(id.to_string()).bind(format!("legacy:{id}")).bind(&now).bind(&now).bind(&now).execute(&mut *tx).await.map_err(storage)?;
            sqlx::query("INSERT INTO agent_instances(id,name,status,capabilities_json,last_heartbeat_at,profile_id,account_id,machine_id,created_at) VALUES(?,?,'online',?,?,?,?,?,?)")
                .bind(id.to_string()).bind(name).bind(json!(capabilities).to_string()).bind(&now).bind(LEGACY_PROFILE).bind(account_id).bind(id.to_string()).bind(&now).execute(&mut *tx).await.map_err(storage)?;
            id
        };
        tx.commit().await.map_err(storage)?;
        self.get_agent(id).await
    }

    /// A name remains the M1 registration key. Re-registration may refresh a
    /// worker, but conflicting identity links cannot silently repoint old work.
    pub async fn register_agent_instance(
        &self,
        input: RegisterAgentInstance,
    ) -> Result<AgentInstance, DomainError> {
        nonempty(&input.name, "agent name")?;
        let profile = self.get_profile(input.profile_id).await?;
        if let Some(id) = input.account_id {
            let account = self.get_account(id).await?;
            if !account.provider.is_empty()
                && !profile.provider.is_empty()
                && account.provider != profile.provider
            {
                return Err(DomainError::InvalidInput(
                    "account/provider mismatch".into(),
                ));
            }
        }
        if let Some(id) = input.machine_id {
            self.get_machine(id).await?;
        }
        let capabilities = input.capabilities.unwrap_or(profile.default_capabilities);
        let now = Utc::now().to_rfc3339();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let existing = sqlx::query("SELECT * FROM agent_instances WHERE name=?")
            .bind(input.name.trim())
            .fetch_optional(&mut *tx)
            .await
            .map_err(storage)?;
        let id = if let Some(row) = existing {
            let instance = row_to_agent(row)?;
            if instance.profile_id != input.profile_id
                || instance.account_id != input.account_id
                || instance.machine_id != input.machine_id
                || instance.external_instance_ref != input.external_instance_ref
            {
                return Err(DomainError::Conflict(
                    "agent name already has different identity links".into(),
                ));
            }
            sqlx::query("UPDATE agent_instances SET status='online',capabilities_json=?,last_heartbeat_at=? WHERE id=?")
                .bind(json!(capabilities).to_string()).bind(&now).bind(instance.id.to_string()).execute(&mut *tx).await.map_err(storage)?;
            instance.id
        } else {
            let id = Uuid::new_v4();
            sqlx::query("INSERT INTO agent_instances(id,name,status,capabilities_json,last_heartbeat_at,profile_id,account_id,machine_id,external_instance_ref,created_at) VALUES(?,?,'online',?,?,?,?,?,?,?)")
                .bind(id.to_string()).bind(input.name.trim()).bind(json!(capabilities).to_string()).bind(&now)
                .bind(input.profile_id.to_string()).bind(input.account_id.map(|x|x.to_string())).bind(input.machine_id.map(|x|x.to_string())).bind(input.external_instance_ref).bind(&now).execute(&mut *tx).await.map_err(storage)?;
            id
        };
        tx.commit().await.map_err(storage)?;
        self.get_agent(id).await
    }

    pub async fn get_agent(&self, id: Id) -> Result<AgentInstance, DomainError> {
        row_to_agent(
            sqlx::query("SELECT * FROM agent_instances WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&self.pool)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("agent {id}")))?,
        )
    }
    pub async fn list_agents(&self) -> Result<Vec<AgentInstance>, DomainError> {
        sqlx::query("SELECT * FROM agent_instances ORDER BY name")
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?
            .into_iter()
            .map(row_to_agent)
            .collect()
    }

    /// Identity ownership is checked before mutation. The heartbeat, host clock,
    /// and optional capacity observation commit together or all roll back.
    pub async fn agent_heartbeat(
        &self,
        id: Id,
        actor: Id,
        input: AgentHeartbeat,
    ) -> Result<AgentInstance, DomainError> {
        own_instance(id, actor)?;
        nonempty(&input.status, "status")?;
        if let Some(ref capacity) = input.capacity {
            validate_capacity(capacity)?;
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let instance = row_to_agent(
            sqlx::query("SELECT * FROM agent_instances WHERE id=?")
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound(format!("agent {id}")))?,
        )?;
        let now = Utc::now();
        sqlx::query("UPDATE agent_instances SET status=?,last_heartbeat_at=? WHERE id=?")
            .bind(input.status)
            .bind(now.to_rfc3339())
            .bind(id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        if let Some(machine) = instance.machine_id {
            // Never move the shared host clock backwards, even if its last seen
            // observation is ahead of this process's clock.
            let previous: Option<String> =
                sqlx::query_scalar("SELECT last_seen_at FROM machines WHERE id=?")
                    .bind(machine.to_string())
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(storage)?;
            // Parse in Rust to retain sub-millisecond precision and handle RFC3339
            // offsets; SQLite julianday would round closely spaced heartbeats.
            let last_seen = parse_opt_dt(previous)?.map_or(now, |seen| seen.max(now));
            sqlx::query("UPDATE machines SET last_seen_at=?,updated_at=? WHERE id=?")
                .bind(last_seen.to_rfc3339())
                .bind(now.to_rfc3339())
                .bind(machine.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
        }
        if let Some(capacity) = input.capacity {
            insert_capacity(&mut tx, id, capacity, now).await?;
        }
        tx.commit().await.map_err(storage)?;
        self.get_agent(id).await
    }

    pub async fn capacity_record(
        &self,
        id: Id,
        actor: Id,
        input: RecordCapacity,
    ) -> Result<CapacitySnapshot, DomainError> {
        own_instance(id, actor)?;
        validate_capacity(&input)?;
        self.get_agent(id).await?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let snapshot = insert_capacity(&mut tx, id, input, Utc::now()).await?;
        tx.commit().await.map_err(storage)?;
        Ok(snapshot)
    }
    pub async fn capacity_latest(&self, id: Id) -> Result<Option<CapacitySnapshot>, DomainError> {
        self.get_agent(id).await?;
        sqlx::query("SELECT * FROM capacity_snapshots WHERE agent_instance_id=? ORDER BY observed_at DESC,rowid DESC LIMIT 1").bind(id.to_string()).fetch_optional(&self.pool).await.map_err(storage)?.map(row_to_capacity).transpose()
    }
    pub async fn capacity_history(&self, id: Id) -> Result<Vec<CapacitySnapshot>, DomainError> {
        self.get_agent(id).await?;
        sqlx::query("SELECT * FROM capacity_snapshots WHERE agent_instance_id=? ORDER BY observed_at DESC,rowid DESC").bind(id.to_string()).fetch_all(&self.pool).await.map_err(storage)?.into_iter().map(row_to_capacity).collect()
    }
    pub async fn agent_fleet(&self) -> Result<Vec<FleetEntry>, DomainError> {
        let mut fleet = Vec::new();
        for instance in self.list_agents().await? {
            fleet.push(FleetEntry {
                profile: self.get_profile(instance.profile_id).await?,
                account: match instance.account_id {
                    Some(id) => Some(self.get_account(id).await?),
                    None => None,
                },
                machine: match instance.machine_id {
                    Some(id) => Some(self.get_machine(id).await?),
                    None => None,
                },
                latest_capacity: self.capacity_latest(instance.id).await?,
                instance,
            });
        }
        Ok(fleet)
    }
}

fn own_instance(id: Id, actor: Id) -> Result<(), DomainError> {
    if id != actor {
        return Err(DomainError::Conflict(
            "instance belongs to another agent".into(),
        ));
    }
    Ok(())
}
fn validate_capacity(input: &RecordCapacity) -> Result<(), DomainError> {
    nonempty(&input.status, "capacity status")?;
    if input.available_slots < 0
        || input.active_assignments < 0
        || input.active_runs < 0
        || input
            .max_concurrency
            .is_some_and(|max| max < 0 || input.available_slots > max)
    {
        return Err(DomainError::InvalidInput(
            "capacity counts must be nonnegative and available_slots <= max_concurrency".into(),
        ));
    }
    Ok(())
}
async fn insert_capacity(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    agent_instance_id: Id,
    capacity: RecordCapacity,
    observed_at: DateTime<Utc>,
) -> Result<CapacitySnapshot, DomainError> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO capacity_snapshots(id,agent_instance_id,status,available_slots,active_assignments,active_runs,max_concurrency,quota_state,details_json,observed_at) VALUES(?,?,?,?,?,?,?,?,?,?)")
        .bind(id.to_string()).bind(agent_instance_id.to_string()).bind(&capacity.status).bind(capacity.available_slots).bind(capacity.active_assignments).bind(capacity.active_runs).bind(capacity.max_concurrency).bind(&capacity.quota_state).bind(capacity.details.to_string()).bind(observed_at.to_rfc3339()).execute(&mut **tx).await.map_err(storage)?;
    Ok(CapacitySnapshot {
        id,
        agent_instance_id,
        capacity,
        observed_at,
    })
}
fn row_to_capacity(row: sqlx::sqlite::SqliteRow) -> Result<CapacitySnapshot, DomainError> {
    Ok(CapacitySnapshot {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        agent_instance_id: parse_id(row.try_get("agent_instance_id").map_err(storage)?)?,
        capacity: RecordCapacity {
            status: row.try_get("status").map_err(storage)?,
            available_slots: row.try_get("available_slots").map_err(storage)?,
            active_assignments: row.try_get("active_assignments").map_err(storage)?,
            active_runs: row.try_get("active_runs").map_err(storage)?,
            max_concurrency: row.try_get("max_concurrency").map_err(storage)?,
            quota_state: row.try_get("quota_state").map_err(storage)?,
            details: parse_json(row.try_get("details_json").map_err(storage)?)?,
        },
        observed_at: parse_dt(row.try_get("observed_at").map_err(storage)?)?,
    })
}
fn row_to_agent(row: sqlx::sqlite::SqliteRow) -> Result<AgentInstance, DomainError> {
    Ok(AgentInstance {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        name: row.try_get("name").map_err(storage)?,
        status: row.try_get("status").map_err(storage)?,
        capabilities: serde_json::from_str(
            &row.try_get::<String, _>("capabilities_json")
                .map_err(storage)?,
        )
        .map_err(storage)?,
        last_heartbeat_at: parse_dt(row.try_get("last_heartbeat_at").map_err(storage)?)?,
        profile_id: parse_id(row.try_get("profile_id").map_err(storage)?)?,
        account_id: parse_opt_id(row.try_get("account_id").map_err(storage)?)?,
        machine_id: parse_opt_id(row.try_get("machine_id").map_err(storage)?)?,
        external_instance_ref: row.try_get("external_instance_ref").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}
