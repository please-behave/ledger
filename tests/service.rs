mod support;

use ledger::{database, http};
use serde_json::{Value, json};
use sqlx::PgPool;
use sqlx::migrate::{Migration, Migrator};
use std::net::SocketAddr;
use std::process::Command;
use support::{get_status, postgres};
use tokio::net::TcpListener;

const UNREACHABLE_DATABASE: &str = "postgres://postgres:none@127.0.0.1:1/postgres";
const MIGRATIONS: Migrator = sqlx::migrate!();

async fn start_service(pool: PgPool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(http::run(listener, pool));
    address
}

async fn status(address: SocketAddr, path: &'static str) -> u16 {
    tokio::task::spawn_blocking(move || get_status(address, path))
        .await
        .expect("the request finishes")
}

fn ledger(url: Option<&str>, arguments: &[&str]) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ledger"));
    command.args(arguments).env_remove("LEDGER_DATABASE_URL");
    if let Some(url) = url {
        command.env("LEDGER_DATABASE_URL", url);
    }
    command.output().expect("run the ledger")
}

fn migration_logs(output: &std::process::Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("a JSON log line"))
        .filter(|line| line["target"] == "ledger")
        .map(|mut line| line["fields"].take())
        .collect()
}

fn assert_migration_logs(output: &std::process::Output, applied: &[&Migration]) {
    assert!(output.status.success(), "{output:?}");
    let mut expected: Vec<_> = applied
        .iter()
        .map(|migration| {
            json!({
                "message": "applied migration",
                "version": migration.version,
                "name": migration.description,
            })
        })
        .collect();
    if applied.is_empty() {
        expected.push(json!({"message": "the schema is already up to date"}));
    }
    expected.push(json!({"message": "the migrations are applied"}));
    assert_eq!(migration_logs(output), expected);
}

// REQ-0003: liveness does not depend on the database.
#[tokio::test(flavor = "multi_thread")]
async fn healthz_answers_without_a_database() {
    let pool = database::connect_lazily(UNREACHABLE_DATABASE).expect("a lazy pool");
    let address = start_service(pool).await;

    assert_eq!(status(address, "/healthz").await, 200);
}

// REQ-0004: readiness fails without a database.
#[tokio::test(flavor = "multi_thread")]
async fn readyz_fails_when_the_database_is_unreachable() {
    let pool = database::connect_lazily(UNREACHABLE_DATABASE).expect("a lazy pool");
    let address = start_service(pool).await;

    assert_eq!(status(address, "/readyz").await, 503);
}

// REQ-0001: starting the service never migrates.
// REQ-0004: readiness follows the schema.
#[tokio::test(flavor = "multi_thread")]
async fn readyz_follows_the_schema_and_starting_never_migrates() {
    let postgres = postgres().await;
    let pool = database::connect_lazily(&postgres.url).expect("a lazy pool");
    let address = start_service(pool.clone()).await;

    assert_eq!(status(address, "/readyz").await, 503);
    let tables: i64 =
        sqlx::query_scalar("select count(*) from pg_tables where schemaname = 'public'")
            .fetch_one(&pool)
            .await
            .expect("count the tables");
    assert_eq!(tables, 0, "the service created no schema");

    database::migrate(&pool).await.expect("migrate");
    assert_eq!(status(address, "/readyz").await, 200);
}

// REQ-0004: a missing migration makes the build not ready.
#[tokio::test(flavor = "multi_thread")]
async fn readyz_fails_when_a_required_migration_is_missing() {
    let postgres = postgres().await;
    let pool = database::connect(&postgres.url).await.expect("connect");
    database::migrate(&pool).await.expect("migrate");
    sqlx::query("delete from _sqlx_migrations")
        .execute(&pool)
        .await
        .expect("forget the migrations");
    let address = start_service(pool).await;

    assert_eq!(status(address, "/readyz").await, 503);
}

// REQ-0001: the migrate subcommand applies the schema.
// REQ-0002: report applied migrations, or that a repeat run has nothing to apply.
#[tokio::test(flavor = "multi_thread")]
async fn the_migrate_subcommand_applies_the_schema_repeatedly() {
    let postgres = postgres().await;

    let first = ledger(Some(&postgres.url), &["migrate"]);
    let second = ledger(Some(&postgres.url), &["migrate"]);

    let migrations: Vec<_> = MIGRATIONS.iter().collect();
    assert_migration_logs(&first, &migrations);
    assert_migration_logs(&second, &[]);
    let pool = database::connect(&postgres.url).await.expect("connect");
    assert!(database::schema_ready(&pool).await);
}

// REQ-0002: report only the migrations applied by this run.
#[tokio::test(flavor = "multi_thread")]
async fn the_migrate_subcommand_reports_only_the_newest_migration() {
    let postgres = postgres().await;
    let pool = database::connect(&postgres.url).await.expect("connect");
    let mut migrations = MIGRATIONS.iter().rev();
    let newest = migrations.next().expect("the newest migration");
    let previous = migrations.next().expect("the preceding migration");
    MIGRATIONS
        .run_to(previous.version, &pool)
        .await
        .expect("apply the preceding schema");

    let output = ledger(Some(&postgres.url), &["migrate"]);

    assert_migration_logs(&output, &[newest]);
    assert!(database::schema_ready(&pool).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_migrate_subcommand_fails_when_a_migration_fails() {
    let postgres = postgres().await;
    let pool = database::connect(&postgres.url).await.expect("connect");
    sqlx::query("create table attestations (id uuid)")
        .execute(&pool)
        .await
        .expect("create a table that conflicts with the first migration");

    let output = ledger(Some(&postgres.url), &["migrate"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot apply the migrations"));
    assert!(migration_logs(&output).is_empty());
}

#[test]
fn the_migrate_subcommand_requires_a_database_url() {
    let output = ledger(None, &["migrate"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("LEDGER_DATABASE_URL"));
}
