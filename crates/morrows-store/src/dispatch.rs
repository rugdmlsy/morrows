use super::*;
use morrows_core::*;
use std::cmp::Ordering;

#[derive(Debug)]
struct CandidateInternal {
    public: DispatchCandidate,
    capacity_observed_at: Option<DateTime<Utc>>,
}

impl Store {
    pub async fn set_dispatch_policy(
        &self,
        task_id: Id,
        input: SetDispatchPolicy,
    ) -> Result<TaskDispatchPolicy, DomainError> {
        self.get_task(task_id).await?;
        validate_policy(&input)?;
        validate_filter_refs(self, &input).await?;
        let now = Utc::now();
        let role = input.role.trim().to_owned();
        let mut capabilities = input.required_capabilities;
        capabilities.sort();
        capabilities.dedup();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        sqlx::query(
            "INSERT INTO task_dispatch_policies(task_id,role,required_capabilities_json,profile_id,account_id,machine_id,heartbeat_ttl_seconds,capacity_ttl_seconds,lease_seconds,enabled,created_at,updated_at)
             VALUES(?,?,?,?,?,?,?,?,?,?,?,?)
             ON CONFLICT(task_id,role) DO UPDATE SET
               required_capabilities_json=excluded.required_capabilities_json,
               profile_id=excluded.profile_id,account_id=excluded.account_id,machine_id=excluded.machine_id,
               heartbeat_ttl_seconds=excluded.heartbeat_ttl_seconds,capacity_ttl_seconds=excluded.capacity_ttl_seconds,
               lease_seconds=excluded.lease_seconds,enabled=excluded.enabled,updated_at=excluded.updated_at",
        )
        .bind(task_id.to_string())
        .bind(&role)
        .bind(json!(capabilities).to_string())
        .bind(input.profile_id.map(|v| v.to_string()))
        .bind(input.account_id.map(|v| v.to_string()))
        .bind(input.machine_id.map(|v| v.to_string()))
        .bind(input.heartbeat_ttl_seconds)
        .bind(input.capacity_ttl_seconds)
        .bind(input.lease_seconds.max(30))
        .bind(input.enabled)
        .bind(now.to_rfc3339())
        .bind(now.to_rfc3339())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        append_event_tx(
            &mut tx,
            "system",
            "dispatcher",
            "task",
            task_id,
            "dispatch.policy.set",
            json!({"role":role,"enabled":input.enabled}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        self.get_dispatch_policy(task_id, &role).await
    }

    pub async fn list_dispatch_policies(&self) -> Result<Vec<TaskDispatchPolicy>, DomainError> {
        let rows = sqlx::query("SELECT * FROM task_dispatch_policies ORDER BY task_id,role")
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        rows.into_iter().map(row_to_policy).collect()
    }

    pub async fn get_dispatch_policy(
        &self,
        task_id: Id,
        role: &str,
    ) -> Result<TaskDispatchPolicy, DomainError> {
        self.get_task(task_id).await?;
        let row = sqlx::query("SELECT * FROM task_dispatch_policies WHERE task_id=? AND role=?")
            .bind(task_id.to_string())
            .bind(role)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| DomainError::NotFound(format!("dispatch policy {task_id}/{role}")))?;
        row_to_policy(row)
    }

    pub async fn dispatch_preview(
        &self,
        task_id: Id,
        role: &str,
    ) -> Result<DispatchPreview, DomainError> {
        let mut conn = self.pool.acquire().await.map_err(storage)?;
        let policy = load_policy_conn(&mut *conn, task_id, role).await?;
        evaluate_dispatch_conn(&mut *conn, task_id, policy, Utc::now()).await
    }

    pub async fn task_dispatch_decisions(
        &self,
        task_id: Id,
    ) -> Result<Vec<DispatchDecision>, DomainError> {
        self.get_task(task_id).await?;
        let rows = sqlx::query(
            "SELECT * FROM dispatch_decisions WHERE task_id=? ORDER BY created_at DESC,id DESC",
        )
        .bind(task_id.to_string())
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?;
        rows.into_iter().map(row_to_decision).collect()
    }

    pub async fn get_dispatch_scheduler_settings(
        &self,
        role: &str,
    ) -> Result<DispatchSchedulerSettings, DomainError> {
        let role = role.trim();
        if role.is_empty() {
            return Err(DomainError::InvalidInput("role cannot be empty".into()));
        }
        let row = sqlx::query("SELECT * FROM dispatch_scheduler_settings WHERE role=?")
            .bind(role)
            .fetch_optional(&self.pool)
            .await
            .map_err(storage)?
            .ok_or_else(|| {
                DomainError::NotFound(format!("dispatch scheduler settings for role {role}"))
            })?;
        row_to_scheduler_settings(row)
    }

    pub async fn list_dispatch_scheduler_settings(
        &self,
    ) -> Result<Vec<DispatchSchedulerSettings>, DomainError> {
        let rows = sqlx::query("SELECT * FROM dispatch_scheduler_settings ORDER BY role")
            .fetch_all(&self.pool)
            .await
            .map_err(storage)?;
        rows.into_iter().map(row_to_scheduler_settings).collect()
    }

    pub async fn set_dispatch_scheduler_settings(
        &self,
        role: &str,
        input: SetDispatchSchedulerSettings,
    ) -> Result<DispatchSchedulerSettings, DomainError> {
        let role = role.trim();
        if role.is_empty() {
            return Err(DomainError::InvalidInput("role cannot be empty".into()));
        }
        if !(1..=300).contains(&input.interval_seconds) {
            return Err(DomainError::InvalidInput(
                "scheduler interval_seconds must be between 1 and 300".into(),
            ));
        }
        if input.auto_launch && role != "executor" {
            return Err(DomainError::InvalidInput(
                "scheduler auto_launch is only supported for executor assignments".into(),
            ));
        }
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            "INSERT INTO dispatch_scheduler_settings(
                role,enabled,interval_seconds,auto_launch,created_at,updated_at
             ) VALUES(?,?,?,?,?,?)
             ON CONFLICT(role) DO UPDATE SET
                enabled=excluded.enabled,
                interval_seconds=excluded.interval_seconds,
                auto_launch=excluded.auto_launch,
                updated_at=excluded.updated_at",
        )
        .bind(role)
        .bind(input.enabled)
        .bind(input.interval_seconds)
        .bind(input.auto_launch)
        .bind(&now)
        .bind(&now)
        .execute(&self.pool)
        .await
        .map_err(storage)?;
        self.get_dispatch_scheduler_settings(role).await
    }

    pub async fn dispatch_task(
        &self,
        task_id: Id,
        role: &str,
    ) -> Result<DispatchOutcome, DomainError> {
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        expire_stale_tx(&mut tx, now).await?;
        let policy = load_policy_conn(&mut tx, task_id, role).await?;
        let preview = evaluate_dispatch_conn(&mut tx, task_id, policy.clone(), now).await?;
        let assignment = if preview.task_dispatchable {
            if let Some(agent) = preview.selected_agent_instance_id {
                Some(
                    insert_dispatch_assignment_tx(
                        &mut tx,
                        task_id,
                        agent,
                        &policy.role,
                        policy.lease_seconds,
                        now,
                    )
                    .await?,
                )
            } else {
                None
            }
        } else {
            None
        };
        let outcome = if assignment.is_some() {
            "assigned"
        } else {
            "no_candidate"
        };
        let decision = insert_decision_tx(
            &mut tx,
            task_id,
            &policy.role,
            outcome,
            preview
                .selected_agent_instance_id
                .filter(|_| assignment.is_some()),
            assignment.as_ref().map(|a| a.id),
            &preview,
            now,
        )
        .await?;
        append_event_tx(
            &mut tx,
            "system",
            "dispatcher",
            "task",
            task_id,
            if assignment.is_some() {
                "dispatch.assigned"
            } else {
                "dispatch.no_candidate"
            },
            json!({"decision_id":decision.id,"role":policy.role,"agent_instance_id":decision.selected_agent_instance_id}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(DispatchOutcome {
            decision,
            assignment,
        })
    }

    pub async fn dispatch_next(&self, role: &str) -> Result<DispatchNextResult, DomainError> {
        self.dispatch_next_inner(role, true).await
    }

    pub async fn dispatch_next_scheduled(
        &self,
        role: &str,
    ) -> Result<DispatchNextResult, DomainError> {
        self.dispatch_next_inner(role, false).await
    }

    async fn dispatch_next_inner(
        &self,
        role: &str,
        record_no_candidate: bool,
    ) -> Result<DispatchNextResult, DomainError> {
        if role.trim().is_empty() {
            return Err(DomainError::InvalidInput("role cannot be empty".into()));
        }
        let role = role.trim();
        let now = Utc::now();
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        expire_stale_tx(&mut tx, now).await?;
        let tasks: Vec<String> = sqlx::query_scalar(
            "SELECT p.task_id FROM task_dispatch_policies p
             JOIN tasks t ON t.id=p.task_id
             WHERE p.role=? AND p.enabled=1 AND t.state IN ('ready','in_progress')
             ORDER BY t.priority DESC,t.created_at ASC,t.id ASC",
        )
        .bind(role)
        .fetch_all(&mut *tx)
        .await
        .map_err(storage)?;

        let mut attempts = Vec::new();
        let mut dispatched = None;
        for raw in tasks {
            let task_id = parse_id(raw)?;
            let policy = load_policy_conn(&mut tx, task_id, role).await?;
            let preview = evaluate_dispatch_conn(&mut tx, task_id, policy.clone(), now).await?;
            let assignment = if preview.task_dispatchable {
                if let Some(agent) = preview.selected_agent_instance_id {
                    Some(
                        insert_dispatch_assignment_tx(
                            &mut tx,
                            task_id,
                            agent,
                            role,
                            policy.lease_seconds,
                            now,
                        )
                        .await?,
                    )
                } else {
                    None
                }
            } else {
                None
            };
            if assignment.is_none() && !record_no_candidate {
                continue;
            }
            let outcome = if assignment.is_some() {
                "assigned"
            } else {
                "no_candidate"
            };
            let decision = insert_decision_tx(
                &mut tx,
                task_id,
                role,
                outcome,
                preview
                    .selected_agent_instance_id
                    .filter(|_| assignment.is_some()),
                assignment.as_ref().map(|a| a.id),
                &preview,
                now,
            )
            .await?;
            append_event_tx(
                &mut tx,
                "system",
                "dispatcher",
                "task",
                task_id,
                if assignment.is_some() {
                    "dispatch.assigned"
                } else {
                    "dispatch.no_candidate"
                },
                json!({"decision_id":decision.id,"role":role,"agent_instance_id":decision.selected_agent_instance_id}),
                None,
            )
            .await?;
            let attempt = DispatchOutcome {
                decision,
                assignment,
            };
            attempts.push(attempt.clone());
            if attempt.assignment.is_some() {
                dispatched = Some(attempt);
                break;
            }
        }
        tx.commit().await.map_err(storage)?;
        Ok(DispatchNextResult {
            dispatched,
            attempts,
        })
    }
}

async fn validate_filter_refs(store: &Store, input: &SetDispatchPolicy) -> Result<(), DomainError> {
    if let Some(id) = input.profile_id {
        store.get_profile(id).await?;
    }
    if let Some(id) = input.account_id {
        store.get_account(id).await?;
    }
    if let Some(id) = input.machine_id {
        store.get_machine(id).await?;
    }
    Ok(())
}

fn validate_policy(input: &SetDispatchPolicy) -> Result<(), DomainError> {
    if input.role.trim().is_empty() {
        return Err(DomainError::InvalidInput("role cannot be empty".into()));
    }
    if input.heartbeat_ttl_seconds <= 0 || input.capacity_ttl_seconds <= 0 {
        return Err(DomainError::InvalidInput(
            "dispatch TTLs must be positive".into(),
        ));
    }
    if input.lease_seconds < 30 {
        return Err(DomainError::InvalidInput(
            "dispatch lease must be at least 30 seconds".into(),
        ));
    }
    if input
        .required_capabilities
        .iter()
        .any(|v| v.trim().is_empty())
    {
        return Err(DomainError::InvalidInput(
            "required capabilities cannot be empty".into(),
        ));
    }
    Ok(())
}

async fn load_policy_conn(
    conn: &mut sqlx::SqliteConnection,
    task_id: Id,
    role: &str,
) -> Result<TaskDispatchPolicy, DomainError> {
    let row = sqlx::query("SELECT * FROM task_dispatch_policies WHERE task_id=? AND role=?")
        .bind(task_id.to_string())
        .bind(role)
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("dispatch policy {task_id}/{role}")))?;
    row_to_policy(row)
}

fn row_to_policy(row: sqlx::sqlite::SqliteRow) -> Result<TaskDispatchPolicy, DomainError> {
    Ok(TaskDispatchPolicy {
        task_id: parse_id(row.try_get("task_id").map_err(storage)?)?,
        role: row.try_get("role").map_err(storage)?,
        required_capabilities: serde_json::from_str(
            &row.try_get::<String, _>("required_capabilities_json")
                .map_err(storage)?,
        )
        .map_err(storage)?,
        profile_id: parse_opt_id(row.try_get("profile_id").map_err(storage)?)?,
        account_id: parse_opt_id(row.try_get("account_id").map_err(storage)?)?,
        machine_id: parse_opt_id(row.try_get("machine_id").map_err(storage)?)?,
        heartbeat_ttl_seconds: row.try_get("heartbeat_ttl_seconds").map_err(storage)?,
        capacity_ttl_seconds: row.try_get("capacity_ttl_seconds").map_err(storage)?,
        lease_seconds: row.try_get("lease_seconds").map_err(storage)?,
        enabled: row.try_get("enabled").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}

fn row_to_scheduler_settings(
    row: sqlx::sqlite::SqliteRow,
) -> Result<DispatchSchedulerSettings, DomainError> {
    Ok(DispatchSchedulerSettings {
        role: row.try_get("role").map_err(storage)?,
        enabled: row.try_get("enabled").map_err(storage)?,
        interval_seconds: row.try_get("interval_seconds").map_err(storage)?,
        auto_launch: row.try_get("auto_launch").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}

fn row_to_decision(row: sqlx::sqlite::SqliteRow) -> Result<DispatchDecision, DomainError> {
    Ok(DispatchDecision {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        task_id: parse_id(row.try_get("task_id").map_err(storage)?)?,
        role: row.try_get("role").map_err(storage)?,
        outcome: row.try_get("outcome").map_err(storage)?,
        selected_agent_instance_id: parse_opt_id(
            row.try_get("selected_agent_instance_id").map_err(storage)?,
        )?,
        assignment_id: parse_opt_id(row.try_get("assignment_id").map_err(storage)?)?,
        preview: serde_json::from_str(&row.try_get::<String, _>("preview_json").map_err(storage)?)
            .map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
    })
}

async fn evaluate_dispatch_conn(
    conn: &mut sqlx::SqliteConnection,
    task_id: Id,
    policy: TaskDispatchPolicy,
    now: DateTime<Utc>,
) -> Result<DispatchPreview, DomainError> {
    let row = sqlx::query("SELECT state FROM tasks WHERE id=?")
        .bind(task_id.to_string())
        .fetch_optional(&mut *conn)
        .await
        .map_err(storage)?
        .ok_or_else(|| DomainError::NotFound(format!("task {task_id}")))?;
    let state: String = row.try_get("state").map_err(storage)?;
    let mut task_reasons = Vec::new();
    if crate::continuation::predecessor_runtime_active(conn, task_id).await? {
        task_reasons.push("previous_execution_still_stopping".into());
    }
    if state != "ready" && state != "in_progress" {
        task_reasons.push(format!("task_state:{state}"));
    }
    let cleanup_blocked: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE task_id=? AND status IN ('interrupted','cleanup_pending','cancelling'))",
    )
    .bind(task_id.to_string()).fetch_one(&mut *conn).await.map_err(storage)?;
    if cleanup_blocked {
        task_reasons.push("runtime_cleanup_pending".into());
    }
    if !policy.enabled {
        task_reasons.push("policy_disabled".into());
    }
    if policy.role == "executor" {
        let blocked: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task_dependencies d JOIN tasks t ON t.id=d.depends_on_task_id WHERE d.task_id=? AND t.state!='done')",
        )
        .bind(task_id.to_string())
        .fetch_one(&mut *conn)
        .await
        .map_err(storage)?;
        if blocked {
            task_reasons.push("unfinished_dependencies".into());
        }
    }
    let active_for_role: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM assignments WHERE task_id=? AND role=? AND status='active' AND expires_at>?)",
    )
    .bind(task_id.to_string())
    .bind(&policy.role)
    .bind(now.to_rfc3339())
    .fetch_one(&mut *conn)
    .await
    .map_err(storage)?;
    if active_for_role {
        task_reasons.push("active_assignment_exists".into());
    }

    let rows = sqlx::query(
        "SELECT ai.id,ai.name,ai.status,ai.capabilities_json,ai.last_heartbeat_at,
                ai.profile_id,ai.account_id,ai.machine_id,
                cs.status AS capacity_status,cs.available_slots,cs.active_assignments AS observed_active_assignments,
                cs.active_runs,cs.max_concurrency,cs.quota_state,cs.observed_at,
                (SELECT COUNT(*) FROM assignments a
                 WHERE a.agent_instance_id=ai.id AND a.status='active' AND a.expires_at>?) AS current_active_assignments
         FROM agent_instances ai
         LEFT JOIN capacity_snapshots cs ON cs.rowid=(
             SELECT c.rowid FROM capacity_snapshots c
             WHERE c.agent_instance_id=ai.id
             ORDER BY c.observed_at DESC,c.rowid DESC LIMIT 1
         )
         ORDER BY ai.id ASC",
    )
    .bind(now.to_rfc3339())
    .fetch_all(&mut *conn)
    .await
    .map_err(storage)?;

    let mut candidates = Vec::new();
    for row in rows {
        let agent_id = parse_id(row.try_get("id").map_err(storage)?)?;
        let agent_name: String = row.try_get("name").map_err(storage)?;
        let instance_status: String = row.try_get("status").map_err(storage)?;
        let capabilities: Vec<String> = serde_json::from_str(
            &row.try_get::<String, _>("capabilities_json")
                .map_err(storage)?,
        )
        .map_err(storage)?;
        let heartbeat = parse_dt(row.try_get("last_heartbeat_at").map_err(storage)?)?;
        let profile_id = parse_id(row.try_get("profile_id").map_err(storage)?)?;
        let account_id = parse_opt_id(row.try_get("account_id").map_err(storage)?)?;
        let machine_id = parse_opt_id(row.try_get("machine_id").map_err(storage)?)?;
        let capacity_status: Option<String> = row.try_get("capacity_status").map_err(storage)?;
        let available_slots: Option<i64> = row.try_get("available_slots").map_err(storage)?;
        let observed_active: Option<i64> = row
            .try_get("observed_active_assignments")
            .map_err(storage)?;
        let max_concurrency: Option<i64> = row.try_get("max_concurrency").map_err(storage)?;
        let quota_state: Option<String> = row.try_get("quota_state").map_err(storage)?;
        let capacity_observed_at = parse_opt_dt(row.try_get("observed_at").map_err(storage)?)?;
        let current_active: i64 = row.try_get("current_active_assignments").map_err(storage)?;

        let heartbeat_age = (now - heartbeat).num_seconds().max(0);
        let capacity_age = capacity_observed_at.map(|seen| (now - seen).num_seconds().max(0));
        let mut reasons = Vec::new();
        if instance_status.to_ascii_lowercase() != "online" {
            reasons.push(format!("instance_status:{instance_status}"));
        }
        for required in &policy.required_capabilities {
            if !capabilities.iter().any(|have| have == required) {
                reasons.push(format!("missing_capability:{required}"));
            }
        }
        if policy
            .profile_id
            .is_some_and(|required| required != profile_id)
        {
            reasons.push("profile_mismatch".into());
        }
        if policy
            .account_id
            .is_some_and(|required| Some(required) != account_id)
        {
            reasons.push("account_mismatch".into());
        }
        if policy
            .machine_id
            .is_some_and(|required| Some(required) != machine_id)
        {
            reasons.push("machine_mismatch".into());
        }
        if heartbeat_age > policy.heartbeat_ttl_seconds {
            reasons.push("stale_heartbeat".into());
        }

        let mut effective_slots = 0;
        match (
            available_slots,
            observed_active,
            capacity_observed_at,
            capacity_status.as_deref(),
        ) {
            (Some(reported), Some(observed), Some(_), Some(status)) => {
                if capacity_age.unwrap_or(i64::MAX) > policy.capacity_ttl_seconds {
                    reasons.push("stale_capacity".into());
                }
                let status_lower = status.to_ascii_lowercase();
                if matches!(
                    status_lower.as_str(),
                    "blocked" | "throttled" | "unavailable" | "offline"
                ) {
                    reasons.push(format!("capacity_status:{status}"));
                }
                if let Some(quota) = quota_state.as_deref() {
                    let q = quota.to_ascii_lowercase();
                    if matches!(q.as_str(), "usage_limited" | "exhausted" | "blocked") {
                        reasons.push(format!("quota_state:{quota}"));
                    }
                }
                let added_since_observation = (current_active - observed).max(0);
                effective_slots = (reported - added_since_observation).max(0);
                if let Some(maximum) = max_concurrency {
                    effective_slots = effective_slots.min((maximum - current_active).max(0));
                }
                if effective_slots <= 0 {
                    reasons.push("no_effective_slots".into());
                }
            }
            _ => reasons.push("missing_capacity".into()),
        }

        candidates.push(CandidateInternal {
            public: DispatchCandidate {
                agent_instance_id: agent_id,
                agent_name,
                eligible: reasons.is_empty(),
                effective_slots,
                current_active_assignments: current_active,
                heartbeat_age_seconds: heartbeat_age,
                capacity_age_seconds: capacity_age,
                capacity_status,
                quota_state,
                reasons,
            },
            capacity_observed_at,
        });
    }

    let continuation = if policy.role == "executor" {
        crate::continuation::load_policy(conn, task_id)
            .await?
            .filter(|p| p.enabled)
    } else {
        None
    };
    if let Some(chain) = &continuation {
        let attempted: Vec<String> = sqlx::query_scalar("SELECT DISTINCT agent_instance_id FROM assignments WHERE task_id=? AND role='executor'")
            .bind(task_id.to_string()).fetch_all(&mut *conn).await.map_err(storage)?;
        for candidate in &mut candidates {
            if !chain
                .agent_ids
                .contains(&candidate.public.agent_instance_id)
            {
                candidate
                    .public
                    .reasons
                    .push("outside_continuation_candidates".into());
            } else if attempted.contains(&candidate.public.agent_instance_id.to_string()) {
                candidate
                    .public
                    .reasons
                    .push("already_attempted_this_task".into());
            }
            candidate.public.eligible = candidate.public.reasons.is_empty();
        }
    }
    candidates.sort_by(|a, b| match (a.public.eligible, b.public.eligible) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        _ if continuation.is_some() => {
            let ids = &continuation.as_ref().unwrap().agent_ids;
            let position = |id| ids.iter().position(|v| *v == id).unwrap_or(usize::MAX);
            position(a.public.agent_instance_id).cmp(&position(b.public.agent_instance_id))
        }
        _ => b
            .public
            .effective_slots
            .cmp(&a.public.effective_slots)
            .then_with(|| {
                a.public
                    .current_active_assignments
                    .cmp(&b.public.current_active_assignments)
            })
            .then_with(|| b.capacity_observed_at.cmp(&a.capacity_observed_at))
            .then_with(|| a.public.agent_instance_id.cmp(&b.public.agent_instance_id)),
    });
    let task_dispatchable = task_reasons.is_empty();
    let selected = if task_dispatchable {
        candidates
            .iter()
            .find(|candidate| candidate.public.eligible)
            .map(|candidate| candidate.public.agent_instance_id)
    } else {
        None
    };
    Ok(DispatchPreview {
        task_id,
        role: policy.role.clone(),
        policy,
        task_dispatchable,
        task_reasons,
        candidates: candidates.into_iter().map(|c| c.public).collect(),
        selected_agent_instance_id: selected,
    })
}

async fn expire_stale_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    now: DateTime<Utc>,
) -> Result<(), DomainError> {
    let rows = sqlx::query(
        "SELECT id,task_id,agent_instance_id FROM assignments
         WHERE status='active' AND expires_at<=?
           AND NOT EXISTS(SELECT 1 FROM runs WHERE runs.assignment_id=assignments.id
                           AND runs.status IN ('interrupted','cleanup_pending','cancelling'))",
    )
    .bind(now.to_rfc3339())
    .fetch_all(&mut **tx)
    .await
    .map_err(storage)?;
    for row in rows {
        let id = parse_id(row.try_get("id").map_err(storage)?)?;
        let task_id = parse_id(row.try_get("task_id").map_err(storage)?)?;
        let agent_id = parse_id(row.try_get("agent_instance_id").map_err(storage)?)?;
        let result = sqlx::query(
            "UPDATE assignments SET status='expired',released_at=?,release_reason='lease_expired'
             WHERE id=? AND status='active'",
        )
        .bind(now.to_rfc3339())
        .bind(id.to_string())
        .execute(&mut **tx)
        .await
        .map_err(storage)?;
        if result.rows_affected() == 1 {
            append_event_tx(
                tx,
                "system",
                "dispatcher",
                "task",
                task_id,
                "assignment.expired",
                json!({"assignment_id":id,"agent_instance_id":agent_id}),
                None,
            )
            .await?;
        }
    }
    Ok(())
}

async fn insert_dispatch_assignment_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: Id,
    agent_id: Id,
    role: &str,
    lease_seconds: i64,
    now: DateTime<Utc>,
) -> Result<Assignment, DomainError> {
    let id = Uuid::new_v4();
    let expires = now + Duration::seconds(lease_seconds.max(30));
    let result = sqlx::query(
        "INSERT INTO assignments(id,task_id,role,agent_instance_id,status,acquired_at,expires_at,renewed_at)
         SELECT ?,id,?,?,'active',?,?,? FROM tasks
         WHERE id=? AND state IN ('ready','in_progress')",
    )
    .bind(id.to_string())
    .bind(role)
    .bind(agent_id.to_string())
    .bind(now.to_rfc3339())
    .bind(expires.to_rfc3339())
    .bind(now.to_rfc3339())
    .bind(task_id.to_string())
    .execute(&mut **tx)
    .await;
    match result {
        Ok(r) if r.rows_affected() == 1 => {}
        Ok(_) => return Err(DomainError::Conflict("task is not dispatchable".into())),
        Err(e) if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("2067") => {
            return Err(DomainError::Conflict(
                "task role already has an active assignment".into(),
            ));
        }
        Err(e) => return Err(storage(e)),
    }
    sqlx::query(
        "UPDATE tasks SET state=CASE WHEN state='ready' THEN 'in_progress' ELSE state END,updated_at=? WHERE id=?",
    )
    .bind(now.to_rfc3339())
    .bind(task_id.to_string())
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    append_event_tx(
        tx,
        "agent_instance",
        &agent_id.to_string(),
        "task",
        task_id,
        "assignment.claimed",
        json!({"assignment_id":id,"role":role,"expires_at":expires,"source":"dispatcher"}),
        None,
    )
    .await?;
    Ok(Assignment {
        id,
        task_id,
        role: role.into(),
        agent_instance_id: agent_id,
        status: "active".into(),
        acquired_at: now,
        expires_at: expires,
        renewed_at: now,
    })
}

async fn insert_decision_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    task_id: Id,
    role: &str,
    outcome: &str,
    selected: Option<Id>,
    assignment_id: Option<Id>,
    preview: &DispatchPreview,
    now: DateTime<Utc>,
) -> Result<DispatchDecision, DomainError> {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO dispatch_decisions(id,task_id,role,outcome,selected_agent_instance_id,assignment_id,preview_json,created_at)
         VALUES(?,?,?,?,?,?,?,?)",
    )
    .bind(id.to_string())
    .bind(task_id.to_string())
    .bind(role)
    .bind(outcome)
    .bind(selected.map(|v| v.to_string()))
    .bind(assignment_id.map(|v| v.to_string()))
    .bind(serde_json::to_string(preview).map_err(storage)?)
    .bind(now.to_rfc3339())
    .execute(&mut **tx)
    .await
    .map_err(storage)?;
    Ok(DispatchDecision {
        id,
        task_id,
        role: role.into(),
        outcome: outcome.into(),
        selected_agent_instance_id: selected,
        assignment_id,
        preview: preview.clone(),
        created_at: now,
    })
}
