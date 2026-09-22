use ac_core::*;
use ac_store::Store;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

fn input<T: DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).unwrap()
}
fn capacity() -> RecordCapacity {
    input(
        json!({"status":"ready","available_slots":2,"active_assignments":1,"active_runs":1,"max_concurrency":3,"quota_state":"unknown","details":{"source":"manual"}}),
    )
}

#[tokio::test]
async fn normalized_and_legacy_registration_preserve_identity() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let p = store.register_profile(input(json!({"name":"Codex","provider":"openai","kind":"coding","default_capabilities":["code"],"metadata":{"version":1}}))).await.unwrap();
    let a = store
        .register_account(input(
            json!({"provider":"openai","label":"personal","external_account_ref":"public-ref"}),
        ))
        .await
        .unwrap();
    let m = store
        .register_machine(input(
            json!({"name":"mac","hostname":"mac.local","os":"macos","arch":"arm64"}),
        ))
        .await
        .unwrap();
    assert_eq!(
        store
            .register_profile(input(json!({"name":"Codex","provider":"openai"})))
            .await
            .unwrap()
            .id,
        p.id
    );
    assert_eq!(
        store
            .register_account(input(json!({"provider":"openai","label":"personal"})))
            .await
            .unwrap()
            .id,
        a.id
    );
    assert_eq!(
        store
            .register_machine(input(json!({"name":"mac"})))
            .await
            .unwrap()
            .id,
        m.id
    );
    let req: RegisterAgentInstance = input(
        json!({"name":"worker","profile_id":p.id,"account_id":a.id,"machine_id":m.id,"external_instance_ref":"session-1"}),
    );
    let worker = store.register_agent_instance(req.clone()).await.unwrap();
    assert_eq!(worker.profile_id, p.id);
    assert_eq!(worker.account_id, Some(a.id));
    assert_eq!(worker.machine_id, Some(m.id));
    assert_eq!(worker.capabilities, vec!["code"]);
    assert_eq!(worker.external_instance_ref.as_deref(), Some("session-1"));
    assert_eq!(
        store.register_agent_instance(req.clone()).await.unwrap().id,
        worker.id
    );
    let legacy_refresh = store
        .register_agent(" worker ", &["review".into()])
        .await
        .unwrap();
    assert_eq!(legacy_refresh.id, worker.id);
    assert_eq!(legacy_refresh.profile_id, p.id);
    assert_eq!(legacy_refresh.account_id, Some(a.id));
    assert_eq!(legacy_refresh.created_at, worker.created_at);
    let mut other = req.clone();
    other.machine_id = None;
    assert!(matches!(
        store.register_agent_instance(other).await,
        Err(DomainError::Conflict(_))
    ));
    for field in ["profile_id", "account_id", "machine_id"] {
        let mut invalid = json!(req);
        invalid[field] = json!(Uuid::new_v4());
        assert!(matches!(
            store.register_agent_instance(input(invalid)).await,
            Err(DomainError::NotFound(_))
        ));
    }
    let wrong = store
        .register_account(input(json!({"provider":"other","label":"other"})))
        .await
        .unwrap();
    let mut invalid = req;
    invalid.account_id = Some(wrong.id);
    assert!(matches!(
        store.register_agent_instance(invalid).await,
        Err(DomainError::InvalidInput(_))
    ));
    let optional = store
        .register_agent_instance(input(
            json!({"name":"optional","profile_id":p.id,"capabilities":[]}),
        ))
        .await
        .unwrap();
    assert_eq!(optional.account_id, None);
    assert!(optional.capabilities.is_empty());
    let old = store
        .register_agent("legacy", &["test".into()])
        .await
        .unwrap();
    assert_eq!(
        store.register_agent("legacy", &[]).await.unwrap().id,
        old.id
    );
    assert_eq!(
        store.get_profile(old.profile_id).await.unwrap().provider,
        "legacy"
    );
    assert_eq!(
        store
            .get_account(old.account_id.unwrap())
            .await
            .unwrap()
            .label,
        "legacy"
    );
    assert_eq!(
        store
            .get_machine(old.machine_id.unwrap())
            .await
            .unwrap()
            .hostname,
        "unknown"
    );
    let fleet = store.agent_fleet().await.unwrap();
    let entry = fleet.iter().find(|e| e.instance.id == worker.id).unwrap();
    assert_eq!(entry.profile.id, p.id);
    assert_eq!(entry.account.as_ref().unwrap().id, a.id);
    assert_eq!(entry.machine.as_ref().unwrap().id, m.id);
    assert!(entry.latest_capacity.is_none());
    assert_eq!(store.list_profiles().await.unwrap().len(), 2);
    assert_eq!(store.list_accounts().await.unwrap().len(), 3);
    assert_eq!(store.list_machines().await.unwrap().len(), 2);
}

#[tokio::test]
async fn heartbeat_capacity_ownership_validation_and_atomicity() {
    let store = Store::connect("sqlite::memory:").await.unwrap();
    let a = store.register_agent("a", &[]).await.unwrap();
    let b = store.register_agent("b", &[]).await.unwrap();
    let heartbeat = AgentHeartbeat {
        status: "busy".into(),
        capacity: Some(capacity()),
    };
    assert!(matches!(
        store.agent_heartbeat(a.id, b.id, heartbeat.clone()).await,
        Err(DomainError::Conflict(_))
    ));
    assert!(matches!(
        store.capacity_record(a.id, b.id, capacity()).await,
        Err(DomainError::Conflict(_))
    ));
    for field in [
        "available_slots",
        "active_assignments",
        "active_runs",
        "max_concurrency",
    ] {
        let mut bad = json!(capacity());
        bad[field] = json!(-1);
        assert!(matches!(
            store.capacity_record(a.id, a.id, input(bad.clone())).await,
            Err(DomainError::InvalidInput(_))
        ));
        assert!(
            store
                .agent_heartbeat(
                    a.id,
                    a.id,
                    AgentHeartbeat {
                        status: "bad".into(),
                        capacity: Some(input(bad))
                    }
                )
                .await
                .is_err()
        );
    }
    let mut bad = capacity();
    bad.available_slots = 4;
    assert!(
        store
            .agent_heartbeat(
                a.id,
                a.id,
                AgentHeartbeat {
                    status: "bad".into(),
                    capacity: Some(bad)
                }
            )
            .await
            .is_err()
    );
    assert_eq!(
        store.get_agent(a.id).await.unwrap().last_heartbeat_at,
        a.last_heartbeat_at
    );
    assert_eq!(store.get_agent(a.id).await.unwrap().status, "online");
    assert!(store.capacity_history(a.id).await.unwrap().is_empty());
    let updated = store.agent_heartbeat(a.id, a.id, heartbeat).await.unwrap();
    assert!(updated.last_heartbeat_at > a.last_heartbeat_at);
    assert_eq!(updated.status, "busy");
    assert_eq!(
        store
            .get_machine(a.machine_id.unwrap())
            .await
            .unwrap()
            .last_seen_at,
        Some(updated.last_heartbeat_at)
    );
    let first = store.capacity_latest(a.id).await.unwrap().unwrap();
    let mut next = capacity();
    next.available_slots = 0;
    next.max_concurrency = Some(0);
    let second = store.capacity_record(a.id, a.id, next).await.unwrap();
    assert_ne!(first.id, second.id);
    let history = store.capacity_history(a.id).await.unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].id, second.id);
    assert_eq!(history[1].id, first.id);
    assert_eq!(
        store.capacity_latest(a.id).await.unwrap().unwrap().id,
        second.id
    );
    assert_eq!(
        store.agent_fleet().await.unwrap()[0]
            .latest_capacity
            .as_ref()
            .unwrap()
            .id,
        second.id
    );
    assert_eq!(
        store.get_agent(a.id).await.unwrap().last_heartbeat_at,
        updated.last_heartbeat_at
    );
    store
        .agent_heartbeat(
            a.id,
            a.id,
            AgentHeartbeat {
                status: "idle".into(),
                capacity: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(store.capacity_history(a.id).await.unwrap().len(), 2);
    assert!(store.capacity_latest(b.id).await.unwrap().is_none());
    assert!(matches!(
        store.capacity_history(Uuid::new_v4()).await,
        Err(DomainError::NotFound(_))
    ));
    let mut unbounded = capacity();
    unbounded.max_concurrency = None;
    store.capacity_record(a.id, a.id, unbounded).await.unwrap();
}

#[tokio::test]
async fn migration_preserves_m21_uuids_and_references_and_capacity_is_append_only() {
    // Apply the actual first three migrations with their checksums, seed an old
    // database, then let Store::connect perform exactly the production upgrade.
    let dir = std::env::temp_dir().join(format!("ac-m3-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    for file in [
        "0001_init.sql",
        "0002_collaboration.sql",
        "0003_messaging_handoff_acceptance.sql",
    ] {
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .join(file),
            dir.join(file),
        )
        .unwrap();
    }
    let url = format!("sqlite://{}?mode=rwc", dir.join("test.db").display());
    let pool = sqlx::SqlitePool::connect(&url).await.unwrap();
    sqlx::migrate::Migrator::new(dir.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();
    let ids: Vec<String> = (0..11).map(|_| Uuid::new_v4().to_string()).collect();
    let (a, t, assign, run, ctx, artifact, thread, msg, handoff, decision, reply) = (
        &ids[0], &ids[1], &ids[2], &ids[3], &ids[4], &ids[5], &ids[6], &ids[7], &ids[8], &ids[9],
        &ids[10],
    );
    let now = "2026-01-01T00:00:00Z";
    sqlx::raw_sql(&format!("
        INSERT INTO agent_instances(id,name,last_heartbeat_at,capabilities_json) VALUES('{a}','legacy','{now}','[\"code\"]');
        INSERT INTO tasks(id,title,owner_actor_id,state,created_at,updated_at) VALUES('{t}','legacy','human:test','in_progress','{now}','{now}');
        INSERT INTO assignments(id,task_id,role,agent_instance_id,status,acquired_at,expires_at,renewed_at) VALUES('{assign}','{t}','executor','{a}','active','{now}','2099-01-01T00:00:00Z','{now}');
        INSERT INTO runs(id,task_id,assignment_id,agent_instance_id,status,started_at) VALUES('{run}','{t}','{assign}','{a}','running','{now}');
        INSERT INTO context_revisions(id,task_id,version,created_by_actor_id,created_at) VALUES('{ctx}','{t}',1,'human:test','{now}');
        INSERT INTO artifacts(id,task_id,created_by,title,uri,description,created_at,kind) VALUES('{artifact}','{t}','{a}','patch','file:///patch','preserved','{now}','patch');
        INSERT INTO decisions VALUES('{decision}','{t}','{a}','choice','reason','{now}');
        INSERT INTO message_threads VALUES('{thread}','{t}','{a}','discussion','{now}');
        INSERT INTO messages(id,thread_id,created_by,body,created_at,message_type,recipient_agent_instance_id,requires_response) VALUES('{msg}','{thread}','{a}','question','{now}','question','{a}',1);
        INSERT INTO messages(id,thread_id,created_by,body,created_at,reply_to_message_id) VALUES('{reply}','{thread}','{a}','reply','{now}','{msg}');
        INSERT INTO handoffs(id,task_id,source_run_id,created_by,context_revision_id,content_json,created_at,status,accepted_by_run_id) VALUES('{handoff}','{t}','{run}','{a}','{ctx}','{{}}','{now}','accepted','{run}');
        INSERT INTO handoff_artifacts VALUES('{handoff}','{artifact}');
        INSERT INTO handoff_decisions VALUES('{handoff}','{decision}');
    ")).execute(&pool).await.unwrap();
    // Compare every column of every preexisting collaboration row, not just IDs.
    let tables = [
        "tasks",
        "assignments",
        "runs",
        "context_revisions",
        "artifacts",
        "decisions",
        "message_threads",
        "messages",
        "handoffs",
        "handoff_artifacts",
        "handoff_decisions",
    ];
    let mut before = Vec::new();
    for table in tables {
        before.push(dump(&pool, table).await);
    }
    let store = Store::connect(&url).await.unwrap();
    for (table, expected) in tables.into_iter().zip(before) {
        assert_eq!(dump(&pool, table).await, expected, "{table} changed");
    }
    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
    let id = Uuid::parse_str(a).unwrap();
    let agent = store.get_agent(id).await.unwrap();
    assert_eq!(agent.id, id);
    assert_eq!(agent.capabilities, vec!["code"]);
    assert_eq!(agent.created_at, agent.last_heartbeat_at);
    assert!(agent.account_id.is_some() && agent.machine_id.is_some());
    assert_eq!(store.register_agent("legacy", &[]).await.unwrap().id, id);
    assert_eq!(
        store
            .get_run(Uuid::parse_str(run).unwrap())
            .await
            .unwrap()
            .agent_instance_id,
        id
    );
    store
        .checkpoint_run(
            Uuid::parse_str(run).unwrap(),
            id,
            json!({"still_owned":true}),
        )
        .await
        .unwrap();
    let snapshot = store.capacity_record(id, id, capacity()).await.unwrap();
    assert!(
        sqlx::query("UPDATE capacity_snapshots SET available_slots=0")
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM capacity_snapshots")
            .execute(&pool)
            .await
            .is_err()
    );
    for counts in [
        "-1,0,0,NULL",
        "0,-1,0,NULL",
        "0,0,-1,NULL",
        "0,0,0,-1",
        "2,0,0,1",
    ] {
        assert!(sqlx::query(&format!("INSERT INTO capacity_snapshots(id,agent_instance_id,status,available_slots,active_assignments,active_runs,max_concurrency,observed_at) VALUES('{}','{id}','ready',{counts},'{now}')",Uuid::new_v4())).execute(&pool).await.is_err());
    }
    assert!(
        sqlx::query("UPDATE agent_instances SET profile_id=NULL")
            .execute(&pool)
            .await
            .is_err()
    );
    drop(store);
    let reopened = Store::connect(&url).await.unwrap();
    assert_eq!(
        reopened.capacity_latest(id).await.unwrap().unwrap().id,
        snapshot.id
    );
    assert_eq!(reopened.get_agent(id).await.unwrap().id, id);
    drop(reopened);
    pool.close().await;
    let _ = std::fs::remove_dir_all(dir);
}

async fn dump(pool: &sqlx::SqlitePool, table: &str) -> Vec<String> {
    use sqlx::Column;
    let rows = sqlx::query(&format!("SELECT * FROM {table} ORDER BY rowid"))
        .fetch_all(pool)
        .await
        .unwrap();
    rows.iter()
        .map(|row| {
            row.columns()
                .iter()
                .map(|col| {
                    // Cast in SQL below would lose NULL/type distinctions; decode the two
                    // SQLite types present in the M1/M2 collaboration fixture explicitly.
                    if let Ok(v) = row.try_get::<Option<String>, _>(col.name()) {
                        format!("{}:{v:?}", col.name())
                    } else {
                        format!(
                            "{}:{:?}",
                            col.name(),
                            row.try_get::<Option<i64>, _>(col.name()).unwrap()
                        )
                    }
                })
                .collect::<Vec<_>>()
                .join("|")
        })
        .collect()
}
