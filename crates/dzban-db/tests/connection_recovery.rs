use std::sync::Arc;
use std::time::Duration;

use dzban_core::DatabaseConfig;
use dzban_core::application::ApplicationContext;
use dzban_core::health::ProbeState;

#[tokio::test]
#[ignore = "requires PostgreSQL 15 from docker-compose.test.yml"]
async fn a_failed_transaction_is_not_mistaken_for_a_lost_connection() {
    let connection = "host=127.0.0.1 port=5415 user=testuser password=testpass dbname=testdb";
    let context = Arc::new(ApplicationContext::new(dzban_db::adapters::registry()));
    let engine = context
        .start_connect_tab(DatabaseConfig::from_connection_string(connection))
        .wait()
        .await
        .unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    let before = engine.execute("SELECT pg_backend_pid()").await.unwrap();
    engine.execute("BEGIN").await.unwrap();
    assert!(engine.execute("SELECT 1 / 0").await.is_err());
    let checked = context
        .health
        .check_tab(&context, tab.id)
        .wait()
        .await
        .unwrap();
    assert_eq!(checked.state, ProbeState::Error);
    assert!(!checked.reconnected);
    assert_eq!(context.health.snapshot().sessions[0].status.recoveries, 0);
    engine.execute("ROLLBACK").await.unwrap();
    let after = engine.execute("SELECT pg_backend_pid()").await.unwrap();
    assert_eq!(before.result_set.rows, after.result_set.rows);
    context.start_shutdown().unwrap().wait().await.unwrap();
}

#[tokio::test]
#[ignore = "requires PostgreSQL 15 from docker-compose.test.yml"]
async fn terminated_postgres_session_recovers_without_replaying_sql_or_replacing_the_tab() {
    let connection = "host=127.0.0.1 port=5415 user=testuser password=testpass dbname=testdb";
    let context = Arc::new(ApplicationContext::new(dzban_db::adapters::registry()));
    let engine = context
        .start_connect_tab(DatabaseConfig::from_connection_string(connection))
        .wait()
        .await
        .unwrap();
    let tab = context.tabs.session(engine.id()).unwrap();
    tab.document.set_draft("SELECT saved draft".into());
    let before = engine.execute("SELECT pg_backend_pid()").await.unwrap();
    let pid: i32 = before.result_set.rows[0][0].display().parse().unwrap();
    engine.execute("BEGIN").await.unwrap();
    engine
        .execute("CREATE TEMP TABLE dzban_recovery_marker (value integer)")
        .await
        .unwrap();
    let (admin, worker) = tokio_postgres::connect(connection, tokio_postgres::NoTls)
        .await
        .unwrap();
    let worker = tokio::spawn(worker);
    admin
        .query_one("SELECT pg_terminate_backend($1)", &[&pid])
        .await
        .unwrap();
    let mut recovered = false;
    for _ in 0..50 {
        let checked = context
            .health
            .check_tab(&context, tab.id)
            .wait()
            .await
            .unwrap();
        if checked.reconnected {
            assert_eq!(checked.state, ProbeState::Connected);
            recovered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(recovered, "The terminated connection never recovered");
    let after = engine
        .execute("SELECT pg_backend_pid(), to_regclass('pg_temp.dzban_recovery_marker')")
        .await
        .unwrap();
    assert_ne!(after.result_set.rows[0][0].display(), pid.to_string());
    assert_eq!(after.result_set.rows[0][1], dzban_core::ResultCell::Null);
    assert_eq!(context.health.snapshot().sessions[0].status.recoveries, 1);
    let current = context.tabs.find(tab.id).unwrap();
    assert_eq!(current.document.id(), tab.document.id());
    assert_eq!(current.document.draft(), "SELECT saved draft");
    assert_eq!(current.engine.unwrap().id(), engine.id());
    context.start_shutdown().unwrap().wait().await.unwrap();
    drop(admin);
    worker.await.unwrap().unwrap();
}
