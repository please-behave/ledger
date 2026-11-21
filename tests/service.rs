mod support;

use ledger::{database, http};
use sqlx::PgPool;
use std::net::SocketAddr;
use std::process::Command;
use support::{get_status, postgres};
use tokio::net::TcpListener;

const UNREACHABLE_DATABASE: &str = "postgres://postgres:none@127.0.0.1:1/postgres";

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
// REQ-0002: running it again succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn the_migrate_subcommand_applies_the_schema_repeatedly() {
    let postgres = postgres().await;

    let first = ledger(Some(&postgres.url), &["migrate"]);
    let second = ledger(Some(&postgres.url), &["migrate"]);

    assert!(first.status.success(), "{first:?}");
    assert!(second.status.success(), "{second:?}");
    let pool = database::connect(&postgres.url).await.expect("connect");
    assert!(database::schema_ready(&pool).await);
}

#[test]
fn the_migrate_subcommand_requires_a_database_url() {
    let output = ledger(None, &["migrate"]);

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("LEDGER_DATABASE_URL"));
}
