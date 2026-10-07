use sqlx::{Row, SqlitePool};
use std::{fs, path::PathBuf};

async fn apply_through(pool: &SqlitePool, last: &str) {
    let mut files: Vec<PathBuf> =
        fs::read_dir(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().and_then(|value| value.to_str()) == Some("sql"))
            .collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_str().unwrap();
        if name > last {
            break;
        }
        let sql = fs::read_to_string(&path).unwrap();
        sqlx::raw_sql(&sql).execute(pool).await.unwrap();
    }
}

#[tokio::test]
async fn migration_0043_preserves_task_transcript_intake_and_provider_continuity() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    apply_through(&pool, "0042_runtime_scope_subject.sql").await;

    let now = "2026-10-08T00:00:00Z";
    let profile = "10000000-0000-0000-0000-000000000001";
    let account = "10000000-0000-0000-0000-000000000002";
    let machine = "10000000-0000-0000-0000-000000000003";
    let agent = "10000000-0000-0000-0000-000000000004";
    let project = "10000000-0000-0000-0000-000000000005";
    let task = "10000000-0000-0000-0000-000000000006";
    let assignment = "10000000-0000-0000-0000-000000000007";
    let run = "10000000-0000-0000-0000-000000000008";
    let launch_profile = "10000000-0000-0000-0000-000000000009";
    let session = "10000000-0000-0000-0000-00000000000a";
    let human_message = "10000000-0000-0000-0000-00000000000b";
    let agent_message = "10000000-0000-0000-0000-00000000000c";
    let launch = "10000000-0000-0000-0000-00000000000d";
    let delivery = "10000000-0000-0000-0000-00000000000e";
    let session_runtime = "10000000-0000-0000-0000-00000000000f";
    let session_credential = "10000000-0000-0000-0000-000000000010";
    let bridge_credential = "10000000-0000-0000-0000-000000000011";

    sqlx::raw_sql(&format!(
        r#"
        INSERT INTO agent_profiles(id,name,provider,kind,default_capabilities_json,metadata_json,created_at,updated_at)
        VALUES('{profile}','codex','codex','cli','[]','{{}}','{now}','{now}');
        INSERT INTO accounts(id,provider,label,status,metadata_json,created_at,updated_at)
        VALUES('{account}','codex','fixture','active','{{}}','{now}','{now}');
        INSERT INTO machines(id,name,hostname,os,arch,status,metadata_json,created_at,updated_at)
        VALUES('{machine}','fixture-mac','fixture-mac','macos','arm64','online','{{}}','{now}','{now}');
        INSERT INTO agent_instances(
          id,name,status,capabilities_json,last_heartbeat_at,metadata_json,
          profile_id,account_id,machine_id,created_at,display_name
        ) VALUES(
          '{agent}','fixture-agent','online','[]','{now}','{{}}',
          '{profile}','{account}','{machine}','{now}','fixture-agent'
        );
        INSERT INTO projects(id,name,description,status,created_at,updated_at)
        VALUES('{project}','Fixture','', 'active','{now}','{now}');
        INSERT INTO tasks(
          id,project_id,title,description,owner_actor_id,state,priority,created_at,updated_at,assignment_mode
        ) VALUES(
          '{task}','{project}','Session migration','legacy transcript','human:operator','in_progress',0,'{now}','{now}','open'
        );
        INSERT INTO assignments(
          id,task_id,role,agent_instance_id,status,acquired_at,expires_at,renewed_at,phase
        ) VALUES(
          '{assignment}','{task}','executor','{agent}','active','{now}','2099-01-01T00:00:00Z','{now}','human_interview'
        );
        INSERT INTO runs(
          id,task_id,assignment_id,agent_instance_id,external_session_ref,status,started_at
        ) VALUES(
          '{run}','{task}','{assignment}','{agent}',NULL,'running','{now}'
        );
        INSERT INTO launch_profiles(
          id,name,adapter,agent_instance_id,program,default_cwd,enabled,created_at,updated_at
        ) VALUES(
          '{launch_profile}','fixture','codex_cli','{agent}','/bin/echo','/tmp',1,'{now}','{now}'
        );
        INSERT INTO sessions(
          id,agent_instance_id,title,status,created_at,updated_at,project_id,task_id
        ) VALUES(
          '{session}','{agent}','legacy task conversation','open','{now}','{now}','{project}','{task}'
        );
        INSERT INTO session_messages(
          id,session_id,author_type,author_agent_instance_id,body,status,created_at,client_message_id
        ) VALUES(
          '{human_message}','{session}','human',NULL,'Please preserve this question','queued','{now}','human-client-1'
        );
        INSERT INTO session_messages(
          id,session_id,author_type,author_agent_instance_id,body,status,created_at
        ) VALUES(
          '{agent_message}','{session}','agent','{agent}','I will preserve it','delivered','{now}'
        );
        INSERT INTO assignment_intakes(
          assignment_id,task_id,agent_instance_id,project_id,
          project_memory_complete,understanding,constraints_json,plan_json,
          questions_json,unresolved_questions_json,interview_status,
          created_at,updated_at,interview_session_id,conversation_state,
          interview_started_at,final_summary_message_id,confirmation_message_id,converged_at
        ) VALUES(
          '{assignment}','{task}','{agent}','{project}',
          1,'understood','{{}}','[]',
          '[]','[]','approved',
          '{now}','{now}','{session}','converged',
          '{now}','{agent_message}','{human_message}','{now}'
        );
        INSERT INTO launch_attempts(
          id,assignment_id,task_id,agent_instance_id,launch_profile_id,run_id,status,cwd,
          external_session_ref,created_at,started_at,ended_at,session_id
        ) VALUES(
          '{launch}','{assignment}','{task}','{agent}','{launch_profile}','{run}','completed','/tmp',
          'provider-thread-123','{now}','{now}','{now}','{session}'
        );
        INSERT INTO agent_deliveries(
          id,agent_instance_id,task_id,kind,source_id,payload_json,status,created_at
        ) VALUES(
          '{delivery}','{agent}','{task}','session_message','{human_message}',
          '{{"session_id":"{session}","message_id":"{human_message}","body":"Please preserve this question"}}',
          'queued','{now}'
        );
        INSERT INTO session_runtime_attempts(
          id,session_id,agent_instance_id,launch_profile_id,adapter,status,cwd,
          provider_session_ref,created_at,started_at,ended_at
        ) VALUES(
          '{session_runtime}','{session}','{agent}','{launch_profile}','codex_cli','completed','/tmp',
          'legacy-direct-provider','{now}','{now}','{now}'
        );
        INSERT INTO agent_credentials(
          id,agent_instance_id,token_hash,kind,label,run_id,session_id,expires_at,created_at
        ) VALUES(
          '{session_credential}','{agent}','hash-session','session_runtime','legacy session runtime',
          NULL,'{session}','2099-01-01T00:00:00Z','{now}'
        );
        INSERT INTO agent_credentials(
          id,agent_instance_id,token_hash,kind,label,run_id,session_id,expires_at,created_at
        ) VALUES(
          '{bridge_credential}','{agent}','hash-bridge','bridge','bridge',
          NULL,NULL,'2099-01-01T00:00:00Z','{now}'
        );
        "#
    ))
    .execute(&pool)
    .await
    .unwrap();

    let migration = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations/0043_session_dedup.sql"),
    )
    .unwrap();
    sqlx::raw_sql(&migration).execute(&pool).await.unwrap();

    let thread = sqlx::query(
        "SELECT id,task_id,kind,target_agent_instance_id FROM message_threads WHERE id=?",
    )
    .bind(session)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(thread.get::<String, _>("id"), session);
    assert_eq!(thread.get::<String, _>("task_id"), task);
    assert_eq!(thread.get::<String, _>("kind"), "human_agent");
    assert_eq!(
        thread
            .get::<Option<String>, _>("target_agent_instance_id")
            .as_deref(),
        Some(agent)
    );

    let human = sqlx::query(
        "SELECT author_type,body,status,client_message_id,recalled_at FROM messages WHERE id=?",
    )
    .bind(human_message)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(human.get::<String, _>("author_type"), "human");
    assert_eq!(
        human.get::<String, _>("body"),
        "Please preserve this question"
    );
    assert_eq!(human.get::<String, _>("status"), "queued");
    assert_eq!(
        human
            .get::<Option<String>, _>("client_message_id")
            .as_deref(),
        Some("human-client-1")
    );
    assert!(human.get::<Option<String>, _>("recalled_at").is_none());

    let reply = sqlx::query("SELECT author_type,created_by,body FROM messages WHERE id=?")
        .bind(agent_message)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(reply.get::<String, _>("author_type"), "agent");
    assert_eq!(
        reply.get::<Option<String>, _>("created_by").as_deref(),
        Some(agent)
    );
    assert_eq!(reply.get::<String, _>("body"), "I will preserve it");

    let intake = sqlx::query(
        "SELECT interview_thread_id,final_summary_message_id,confirmation_message_id,conversation_state
         FROM assignment_intakes WHERE assignment_id=?",
    )
    .bind(assignment)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        intake
            .get::<Option<String>, _>("interview_thread_id")
            .as_deref(),
        Some(session)
    );
    assert_eq!(
        intake
            .get::<Option<String>, _>("final_summary_message_id")
            .as_deref(),
        Some(agent_message)
    );
    assert_eq!(
        intake
            .get::<Option<String>, _>("confirmation_message_id")
            .as_deref(),
        Some(human_message)
    );
    assert_eq!(intake.get::<String, _>("conversation_state"), "converged");

    let provider_ref: Option<String> =
        sqlx::query_scalar("SELECT provider_conversation_ref FROM runs WHERE id=?")
            .bind(run)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(provider_ref.as_deref(), Some("provider-thread-123"));

    let attempt_columns: Vec<String> = sqlx::query("PRAGMA table_info(launch_attempts)")
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.get::<String, _>("name"))
        .collect();
    assert!(!attempt_columns.iter().any(|name| name == "session_id"));
    assert!(
        !attempt_columns
            .iter()
            .any(|name| name == "external_session_ref")
    );
    assert!(
        !attempt_columns
            .iter()
            .any(|name| name == "provider_conversation_ref")
    );

    let delivery_row =
        sqlx::query("SELECT kind,payload_json,status FROM agent_deliveries WHERE id=?")
            .bind(delivery)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(delivery_row.get::<String, _>("kind"), "task_message");
    let payload: serde_json::Value =
        serde_json::from_str(&delivery_row.get::<String, _>("payload_json")).unwrap();
    assert_eq!(payload["thread_id"], session);
    assert!(payload.get("session_id").is_none());

    let legacy_session_status: String =
        sqlx::query_scalar("SELECT status FROM sessions WHERE id=?")
            .bind(session)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(legacy_session_status, "archived");

    let direct_provider: Option<String> = sqlx::query_scalar(
        "SELECT provider_conversation_ref FROM session_runtime_attempts WHERE id=?",
    )
    .bind(session_runtime)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(direct_provider.as_deref(), Some("legacy-direct-provider"));

    let session_credential_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_credentials WHERE id=?")
            .bind(session_credential)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(session_credential_count, 0);
    let bridge_credential_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_credentials WHERE id=?")
            .bind(bridge_credential)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(bridge_credential_count, 1);

    assert!(
        sqlx::query("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .unwrap()
            .is_empty()
    );
}
