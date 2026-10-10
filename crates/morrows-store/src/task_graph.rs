use super::*;
use crate::memory::task_writer_conn;
use morrows_core::*;
use std::collections::{HashMap, HashSet};

/// Evaluate the entire prerequisite DAG in one snapshot, without recursive async calls.
/// Exclusion propagates through unfinished nodes, so descendants of an unselected
/// branch do not wait forever. Legacy unbound edges always form an ALL barrier;
/// a named member's join mode applies only to that chain's own incoming edges.
pub(super) async fn gate_conn(
    conn: &mut sqlx::SqliteConnection,
    task_id: Id,
) -> Result<TaskGate, DomainError> {
    let tasks = sqlx::query("SELECT t.id,t.state,COALESCE(m.join_mode,'all') AS join_mode FROM tasks t LEFT JOIN task_chain_members m ON m.task_id=t.id")
        .fetch_all(&mut *conn).await.map_err(storage)?;
    let mut states = HashMap::new();
    let mut modes = HashMap::new();
    for row in tasks {
        let id = parse_id(row.try_get("id").map_err(storage)?)?;
        states.insert(id, row.try_get::<String, _>("state").map_err(storage)?);
        modes.insert(
            id,
            if row.try_get::<String, _>("join_mode").map_err(storage)? == "any" {
                JoinMode::Any
            } else {
                JoinMode::All
            },
        );
    }
    if !states.contains_key(&task_id) {
        return Err(DomainError::NotFound(format!("task {task_id}")));
    }
    let rows = sqlx::query("SELECT d.*, (SELECT r.result_json FROM runs r JOIN assignments a ON a.id=r.assignment_id WHERE r.task_id=d.depends_on_task_id AND r.status='completed' AND a.role='executor' ORDER BY r.ended_at DESC,r.rowid DESC LIMIT 1) AS completion_result FROM task_dependencies d ORDER BY d.created_at,d.depends_on_task_id")
        .fetch_all(&mut *conn).await.map_err(storage)?;
    let mut incoming: HashMap<Id, Vec<(EdgeGate, bool)>> = HashMap::new();
    let mut successors = Vec::new();
    for row in rows {
        let to = parse_id(row.try_get("task_id").map_err(storage)?)?;
        let from = parse_id(row.try_get("depends_on_task_id").map_err(storage)?)?;
        if from == task_id {
            successors.push(to);
        }
        let bound = row
            .try_get::<Option<String>, _>("chain_id")
            .map_err(storage)?
            .is_some();
        let condition: EdgeCondition = serde_json::from_str(
            &row.try_get::<String, _>("condition_json")
                .map_err(storage)?,
        )
        .map_err(storage)?;
        let (state, reason) = match states.get(&from).map(String::as_str) {
            Some("cancelled") => (GateState::Excluded, "predecessor_cancelled"),
            Some("done") => {
                let matches = match &condition {
                    EdgeCondition::Unconditional => true,
                    EdgeCondition::ResultEquals { path, value } => {
                        let raw: Option<String> =
                            row.try_get("completion_result").map_err(storage)?;
                        let result = raw
                            .map(|v| serde_json::from_str::<Value>(&v))
                            .transpose()
                            .map_err(storage)?;
                        result.as_ref().and_then(|v| v.pointer(path)) == Some(value)
                    }
                };
                if matches {
                    (GateState::Eligible, "satisfied")
                } else {
                    (GateState::Excluded, "result_condition_not_matched")
                }
            }
            _ => (GateState::Waiting, "predecessor_unfinished"),
        };
        incoming.entry(to).or_default().push((
            EdgeGate {
                predecessor_id: from,
                condition,
                state,
                reason: reason.into(),
            },
            bound,
        ));
    }
    let mut resolved = HashMap::<Id, GateState>::new();
    let mut pending: HashSet<Id> = states.keys().copied().collect();
    while !pending.is_empty() {
        let mut progressed = false;
        for id in pending.clone() {
            let edges = incoming.entry(id).or_default();
            if edges
                .iter()
                .any(|(e, _)| !resolved.contains_key(&e.predecessor_id))
            {
                continue;
            }
            for (edge, _) in edges.iter_mut() {
                if edge.state == GateState::Waiting
                    && resolved[&edge.predecessor_id] == GateState::Excluded
                {
                    edge.state = GateState::Excluded;
                    edge.reason = "predecessor_branch_excluded".into();
                }
            }
            let legacy = combine(
                edges
                    .iter()
                    .filter(|(_, bound)| !bound)
                    .map(|(e, _)| e.state),
                JoinMode::All,
            );
            let named = combine(
                edges
                    .iter()
                    .filter(|(_, bound)| *bound)
                    .map(|(e, _)| e.state),
                modes[&id],
            );
            resolved.insert(id, combine([legacy, named].into_iter(), JoinMode::All));
            pending.remove(&id);
            progressed = true;
        }
        if !progressed {
            return Err(DomainError::Conflict("dependency cycle".into()));
        }
    }
    Ok(TaskGate {
        task_id,
        join_mode: modes[&task_id],
        state: resolved[&task_id],
        predecessors: incoming
            .remove(&task_id)
            .unwrap_or_default()
            .into_iter()
            .map(|(e, _)| e)
            .collect(),
        successors,
    })
}
fn combine(states: impl Iterator<Item = GateState>, mode: JoinMode) -> GateState {
    let values: Vec<_> = states.collect();
    if values.is_empty() {
        return GateState::Eligible;
    }
    match mode {
        JoinMode::All if values.contains(&GateState::Excluded) => GateState::Excluded,
        JoinMode::Any if values.contains(&GateState::Eligible) => GateState::Eligible,
        _ if values.contains(&GateState::Waiting) => GateState::Waiting,
        JoinMode::All => GateState::Eligible,
        JoinMode::Any => GateState::Excluded,
    }
}
pub(super) async fn enforce_gate_conn(
    conn: &mut sqlx::SqliteConnection,
    task_id: Id,
) -> Result<(), DomainError> {
    let gate = gate_conn(conn, task_id).await?;
    if gate.state != GateState::Eligible {
        return Err(DomainError::Conflict(format!(
            "task chain gate: {}",
            serde_json::to_string(&gate).map_err(storage)?
        )));
    }
    Ok(())
}
impl Store {
    pub async fn task_gate(&self, task_id: Id) -> Result<TaskGate, DomainError> {
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let gate = gate_conn(&mut tx, task_id).await?;
        tx.commit().await.map_err(storage)?;
        Ok(gate)
    }
}

// Table names are selected exclusively from these constants, never from request text.
fn tables(chain: bool) -> (&'static str, &'static str, &'static str) {
    if chain {
        ("task_chains", "task_chain_members", "chain_id")
    } else {
        ("task_groups", "task_group_members", "group_id")
    }
}
fn collection_row(row: sqlx::sqlite::SqliteRow) -> Result<TaskCollection, DomainError> {
    Ok(TaskCollection {
        id: parse_id(row.try_get("id").map_err(storage)?)?,
        name: row.try_get("name").map_err(storage)?,
        description: row.try_get("description").map_err(storage)?,
        archived: row.try_get("archived").map_err(storage)?,
        created_at: parse_dt(row.try_get("created_at").map_err(storage)?)?,
        updated_at: parse_dt(row.try_get("updated_at").map_err(storage)?)?,
    })
}
async fn active_collection(
    conn: &mut sqlx::SqliteConnection,
    chain: bool,
    id: Id,
) -> Result<(), DomainError> {
    let archived: bool = sqlx::query_scalar(&format!(
        "SELECT archived FROM {} WHERE id=?",
        tables(chain).0
    ))
    .bind(id.to_string())
    .fetch_optional(conn)
    .await
    .map_err(storage)?
    .ok_or_else(|| DomainError::NotFound("collection".into()))?;
    if archived {
        return Err(DomainError::Conflict("collection is archived".into()));
    }
    Ok(())
}
impl Store {
    pub async fn list_task_collections(
        &self,
        chain: bool,
    ) -> Result<Vec<TaskCollection>, DomainError> {
        sqlx::query(&format!(
            "SELECT * FROM {} ORDER BY created_at,id",
            tables(chain).0
        ))
        .fetch_all(&self.pool)
        .await
        .map_err(storage)?
        .into_iter()
        .map(collection_row)
        .collect()
    }
    pub async fn save_task_collection(
        &self,
        chain: bool,
        id: Option<Id>,
        input: SaveTaskCollection,
    ) -> Result<TaskCollection, DomainError> {
        if input.name.trim().is_empty() {
            return Err(DomainError::InvalidInput("name cannot be empty".into()));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let now = Utc::now().to_rfc3339();
        let table = tables(chain).0;
        let collection_id = id.unwrap_or_else(Uuid::new_v4);
        if id.is_some() {
            let result = sqlx::query(&format!(
                "UPDATE {table} SET name=?,description=?,archived=?,updated_at=? WHERE id=?"
            ))
            .bind(input.name.trim())
            .bind(&input.description)
            .bind(input.archived)
            .bind(&now)
            .bind(collection_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
            if result.rows_affected() == 0 {
                return Err(DomainError::NotFound("collection".into()));
            }
        } else {
            sqlx::query(&format!("INSERT INTO {table}(id,name,description,archived,created_at,updated_at) VALUES(?,?,?,?,?,?)")).bind(collection_id.to_string()).bind(input.name.trim()).bind(&input.description).bind(input.archived).bind(&now).bind(&now).execute(&mut *tx).await.map_err(storage)?;
        }
        append_event_tx(
            &mut tx,
            "operator",
            "operator",
            table,
            collection_id,
            "collection.saved",
            json!(input),
            None,
        )
        .await?;
        let result = collection_row(
            sqlx::query(&format!("SELECT * FROM {table} WHERE id=?"))
                .bind(collection_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?,
        )?;
        tx.commit().await.map_err(storage)?;
        Ok(result)
    }
    /// Employee group management is separate from control-plane group management.
    /// An agent cannot alter legacy operator-created groups or another agent's group.
    pub async fn create_agent_task_group(
        &self,
        agent_id: Id,
        input: SaveTaskCollection,
    ) -> Result<TaskCollection, DomainError> {
        let name = input.name.trim();
        if name.is_empty() || name.chars().count() > 160 {
            return Err(DomainError::InvalidInput(
                "group name must contain 1..160 characters".into(),
            ));
        }
        if input.description.len() > 8192 || input.archived {
            return Err(DomainError::InvalidInput(
                "invalid new group description or archived state".into(),
            ));
        }
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM agent_instances WHERE id=?)")
                .bind(agent_id.to_string())
                .fetch_one(&mut *tx)
                .await
                .map_err(storage)?;
        if !exists {
            return Err(DomainError::NotFound("agent instance".into()));
        }
        let id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        sqlx::query("INSERT INTO task_groups(id,name,description,archived,created_at,updated_at,owner_agent_instance_id) VALUES(?,?,?,0,?,?,?)")
            .bind(id.to_string()).bind(name).bind(&input.description)
            .bind(&now).bind(&now).bind(agent_id.to_string())
            .execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task_groups",
            id,
            "collection.created",
            json!({"name":name}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(TaskCollection {
            id,
            name: name.into(),
            description: input.description,
            archived: false,
            created_at: parse_dt(now.clone())?,
            updated_at: parse_dt(now)?,
        })
    }

    pub async fn change_agent_task_group_member(
        &self,
        agent_id: Id,
        group_id: Id,
        task_id: Id,
        remove: bool,
    ) -> Result<(), DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let row =
            sqlx::query("SELECT owner_agent_instance_id,archived FROM task_groups WHERE id=?")
                .bind(group_id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound("task group".into()))?;
        let owner: Option<String> = row.try_get("owner_agent_instance_id").map_err(storage)?;
        let archived: bool = row.try_get("archived").map_err(storage)?;
        if archived {
            return Err(DomainError::Conflict("group is archived".into()));
        }
        if owner.as_deref() != Some(agent_id.to_string().as_str()) {
            return Err(DomainError::Conflict(
                "only the creating agent may modify this group".into(),
            ));
        }
        task_writer_conn(&mut tx, task_id, agent_id).await?;
        if remove {
            sqlx::query("DELETE FROM task_group_members WHERE group_id=? AND task_id=?")
                .bind(group_id.to_string())
                .bind(task_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
        } else {
            sqlx::query("INSERT OR IGNORE INTO task_group_members(group_id,task_id) VALUES(?,?)")
                .bind(group_id.to_string())
                .bind(task_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
        }
        append_event_tx(
            &mut tx,
            "agent_instance",
            &agent_id.to_string(),
            "task_groups",
            group_id,
            "collection.member_changed",
            json!({"task_id":task_id,"removed":remove}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(())
    }

    /// Archive instead of destroying history. Archiving a chain never disables gates.
    pub async fn archive_task_collection(&self, chain: bool, id: Id) -> Result<(), DomainError> {
        let table = tables(chain).0;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        let result = sqlx::query(&format!(
            "UPDATE {table} SET archived=1,updated_at=? WHERE id=?"
        ))
        .bind(Utc::now().to_rfc3339())
        .bind(id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        if result.rows_affected() == 0 {
            return Err(DomainError::NotFound("collection".into()));
        }
        append_event_tx(
            &mut tx,
            "operator",
            "operator",
            table,
            id,
            "collection.archived",
            json!({}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(())
    }
    pub async fn task_collection_detail(&self, chain: bool, id: Id) -> Result<Value, DomainError> {
        let (table, members, key) = tables(chain);
        let mut tx = self.pool.begin().await.map_err(storage)?;
        let collection = collection_row(
            sqlx::query(&format!("SELECT * FROM {table} WHERE id=?"))
                .bind(id.to_string())
                .fetch_optional(&mut *tx)
                .await
                .map_err(storage)?
                .ok_or_else(|| DomainError::NotFound("collection".into()))?,
        )?;
        let tasks=sqlx::query(&format!("SELECT t.* FROM tasks t JOIN {members} m ON m.task_id=t.id WHERE m.{key}=? ORDER BY t.created_at,t.id")).bind(id.to_string()).fetch_all(&mut *tx).await.map_err(storage)?.into_iter().map(row_to_task).collect::<Result<Vec<_>,_>>()?;
        let mut gates = Vec::new();
        for task in &tasks {
            gates.push(gate_conn(&mut tx, task.id).await?);
        }
        let edges = if chain {
            sqlx::query("SELECT * FROM task_dependencies WHERE chain_id=? ORDER BY task_id,depends_on_task_id").bind(id.to_string()).fetch_all(&mut *tx).await.map_err(storage)?.into_iter().map(|r| Ok(json!({"task_id":r.try_get::<String,_>("task_id").map_err(storage)?,"depends_on_task_id":r.try_get::<String,_>("depends_on_task_id").map_err(storage)?,"condition":serde_json::from_str::<Value>(&r.try_get::<String,_>("condition_json").map_err(storage)?).map_err(storage)?}))).collect::<Result<Vec<Value>,DomainError>>()?
        } else {
            vec![]
        };
        tx.commit().await.map_err(storage)?;
        Ok(json!({"collection":collection,"members":tasks,"gates":gates,"edges":edges}))
    }
    pub async fn set_collection_member(
        &self,
        chain: bool,
        id: Id,
        task_id: Id,
        join: JoinMode,
        remove: bool,
    ) -> Result<(), DomainError> {
        let (_, members, key) = tables(chain);
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        active_collection(&mut tx, chain, id).await?;
        if remove {
            if chain {
                let used: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task_dependencies WHERE chain_id=? AND (task_id=? OR depends_on_task_id=?))").bind(id.to_string()).bind(task_id.to_string()).bind(task_id.to_string()).fetch_one(&mut *tx).await.map_err(storage)?;
                if used {
                    return Err(DomainError::Conflict(
                        "remove the member's chain edges first".into(),
                    ));
                }
            }
            sqlx::query(&format!(
                "DELETE FROM {members} WHERE {key}=? AND task_id=?"
            ))
            .bind(id.to_string())
            .bind(task_id.to_string())
            .execute(&mut *tx)
            .await
            .map_err(storage)?;
        } else if chain {
            sqlx::query("INSERT INTO task_chain_members(chain_id,task_id,join_mode) VALUES(?,?,?) ON CONFLICT(chain_id,task_id) DO UPDATE SET join_mode=excluded.join_mode").bind(id.to_string()).bind(task_id.to_string()).bind(if join==JoinMode::Any {"any"} else {"all"}).execute(&mut *tx).await.map_err(storage)?;
        } else {
            sqlx::query("INSERT OR IGNORE INTO task_group_members(group_id,task_id) VALUES(?,?)")
                .bind(id.to_string())
                .bind(task_id.to_string())
                .execute(&mut *tx)
                .await
                .map_err(storage)?;
        }
        append_event_tx(
            &mut tx,
            "operator",
            "operator",
            tables(chain).0,
            id,
            "collection.member_changed",
            json!({"task_id":task_id,"removed":remove,"join_mode":join}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(())
    }
    /// A write lock covers membership and global reachability checks and edge mutation.
    /// Cycles through legacy dependencies are rejected just like named-chain cycles.
    pub async fn save_chain_edge(
        &self,
        chain_id: Id,
        input: SaveChainEdge,
    ) -> Result<(), DomainError> {
        validate_condition(&input.condition)?;
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        active_collection(&mut tx, true, chain_id).await?;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_chain_members WHERE chain_id=? AND task_id IN (?,?)",
        )
        .bind(chain_id.to_string())
        .bind(input.task_id.to_string())
        .bind(input.depends_on_task_id.to_string())
        .fetch_one(&mut *tx)
        .await
        .map_err(storage)?;
        if count != 2 {
            return Err(DomainError::InvalidInput(
                "edge requires two distinct chain members".into(),
            ));
        }
        let cycle: bool=sqlx::query_scalar("WITH RECURSIVE reachable(id) AS (SELECT depends_on_task_id FROM task_dependencies WHERE task_id=? UNION SELECT d.depends_on_task_id FROM task_dependencies d JOIN reachable r ON d.task_id=r.id) SELECT EXISTS(SELECT 1 FROM reachable WHERE id=?)").bind(input.depends_on_task_id.to_string()).bind(input.task_id.to_string()).fetch_one(&mut *tx).await.map_err(storage)?;
        if cycle {
            return Err(DomainError::Conflict("dependency cycle".into()));
        }
        let existing: Option<Option<String>> = sqlx::query_scalar(
            "SELECT chain_id FROM task_dependencies WHERE task_id=? AND depends_on_task_id=?",
        )
        .bind(input.task_id.to_string())
        .bind(input.depends_on_task_id.to_string())
        .fetch_optional(&mut *tx)
        .await
        .map_err(storage)?;
        if existing.is_some_and(|v| v.as_deref() != Some(&chain_id.to_string())) {
            return Err(DomainError::Conflict(
                "edge already exists outside this chain".into(),
            ));
        }
        sqlx::query("INSERT INTO task_dependencies(task_id,depends_on_task_id,created_by,created_by_actor_id,created_at,chain_id,condition_json) VALUES(?,?,NULL,?,?,?,?) ON CONFLICT(task_id,depends_on_task_id) DO UPDATE SET condition_json=excluded.condition_json, chain_id=excluded.chain_id, created_by_actor_id=excluded.created_by_actor_id")
            .bind(input.task_id.to_string()).bind(input.depends_on_task_id.to_string()).bind("operator:control-plane").bind(Utc::now().to_rfc3339()).bind(chain_id.to_string()).bind(serde_json::to_string(&input.condition).map_err(storage)?).execute(&mut *tx).await.map_err(storage)?;
        append_event_tx(
            &mut tx,
            "operator",
            "operator",
            "task_chain",
            chain_id,
            "chain.edge_saved",
            json!(input),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(())
    }
    pub async fn remove_chain_edge(
        &self,
        chain_id: Id,
        task_id: Id,
        predecessor: Id,
    ) -> Result<(), DomainError> {
        let mut tx = self
            .pool
            .begin_with("BEGIN IMMEDIATE")
            .await
            .map_err(storage)?;
        active_collection(&mut tx, true, chain_id).await?;
        let result = sqlx::query(
            "DELETE FROM task_dependencies WHERE chain_id=? AND task_id=? AND depends_on_task_id=?",
        )
        .bind(chain_id.to_string())
        .bind(task_id.to_string())
        .bind(predecessor.to_string())
        .execute(&mut *tx)
        .await
        .map_err(storage)?;
        if result.rows_affected() == 0 {
            return Err(DomainError::NotFound("chain edge".into()));
        }
        append_event_tx(
            &mut tx,
            "operator",
            "operator",
            "task_chain",
            chain_id,
            "chain.edge_removed",
            json!({"task_id":task_id,"depends_on_task_id":predecessor}),
            None,
        )
        .await?;
        tx.commit().await.map_err(storage)?;
        Ok(())
    }
    /// Each member has its own transaction and intake. A failed or blocked member
    /// cannot roll back successful siblings, and no shared Run is manufactured.
    pub async fn assign_task_group(
        &self,
        input: GroupBatchAssign,
    ) -> Result<Vec<BatchAssignmentResult>, DomainError> {
        self.get_agent(input.agent_instance_id).await?;
        let mut tx = self.pool.begin().await.map_err(storage)?;
        active_collection(&mut tx, false, input.group_id).await?;
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT task_id FROM task_group_members WHERE group_id=? ORDER BY task_id",
        )
        .bind(input.group_id.to_string())
        .fetch_all(&mut *tx)
        .await
        .map_err(storage)?;
        tx.commit().await.map_err(storage)?;
        let mut results = Vec::new();
        for raw in ids {
            let task_id = parse_id(raw)?;
            let mut tx = self
                .pool
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(storage)?;
            active_collection(&mut tx, false, input.group_id).await?;
            let gate = gate_conn(&mut tx, task_id).await?;
            if gate.state != GateState::Eligible {
                results.push(BatchAssignmentResult {
                    task_id,
                    status: "blocked".into(),
                    assignment: None,
                    gate,
                    error: None,
                });
                tx.rollback().await.map_err(storage)?;
                continue;
            }
            match claim_task_tx(
                &mut tx,
                task_id,
                input.agent_instance_id,
                "executor",
                input.lease_seconds,
                true,
            )
            .await
            {
                Ok(assignment) => {
                    append_event_tx(
                        &mut tx,
                        "operator",
                        "operator",
                        "task",
                        task_id,
                        "group.assigned",
                        json!({"group_id":input.group_id,"assignment_id":assignment.id}),
                        None,
                    )
                    .await?;
                    tx.commit().await.map_err(storage)?;
                    results.push(BatchAssignmentResult {
                        task_id,
                        status: "assigned".into(),
                        assignment: Some(assignment),
                        gate,
                        error: None,
                    });
                }
                Err(error) => {
                    tx.rollback().await.map_err(storage)?;
                    results.push(BatchAssignmentResult {
                        task_id,
                        status: "failed".into(),
                        assignment: None,
                        gate,
                        error: Some(error.to_string()),
                    });
                }
            }
        }
        Ok(results)
    }
}
fn validate_condition(condition: &EdgeCondition) -> Result<(), DomainError> {
    if let EdgeCondition::ResultEquals { path, .. } = condition {
        let valid_escapes = path
            .split('~')
            .skip(1)
            .all(|s| s.starts_with('0') || s.starts_with('1'));
        if (!path.is_empty() && !path.starts_with('/')) || !valid_escapes {
            return Err(DomainError::InvalidInput(
                "result path must be an RFC 6901 JSON Pointer".into(),
            ));
        }
    }
    Ok(())
}
