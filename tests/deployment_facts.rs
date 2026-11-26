mod support;

use ledger::http;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use support::{migrated_pool, post, postgres};
use tokio::net::TcpListener;

const PATH: &str = "/v0/facts/deployments";
const SENDER: &str = "github.com/owner/infra";
const SUBJECT: &str = "github.com/owner/software";
const TARGET: &str = "production";
const COMMIT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn deployment() -> Value {
    json!({
        "target": TARGET,
        "repository": SUBJECT,
        "version": "v1",
        "sender": SENDER,
        "auth_method": "github_oidc",
    })
}

async fn start_service(pool: PgPool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(http::run(listener, pool));
    address
}

async fn submit(address: SocketAddr, path: &'static str, body: String) -> (u16, String) {
    tokio::task::spawn_blocking(move || post(address, path, &body))
        .await
        .expect("the request finishes")
}

async fn register(address: SocketAddr, sender: &str, target: &str) {
    let body = json!({"sender": sender, "targets": [{"name": target}]});
    assert_eq!(
        submit(address, "/v0/target-registrations", body.to_string())
            .await
            .0,
        200
    );
}

async fn receipt(address: SocketAddr, report: &Value) -> (u16, Value) {
    let (status, body) = submit(address, PATH, report.to_string()).await;
    (status, serde_json::from_str(&body).expect("a JSON receipt"))
}

async fn event_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("select count(*) from deployment_events")
        .fetch_one(pool)
        .await
        .expect("count deployment events")
}

// REQ-0053: changes and rollbacks append events, retries leave history intact.
#[tokio::test]
async fn records_changes_and_rollbacks_without_requiring_release_facts() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    register(address, SENDER, TARGET).await;
    let created = (201, json!({"outcome": "created"}));
    let unchanged = (200, json!({"outcome": "unchanged"}));
    let earliest: String = sqlx::query_scalar("select clock_timestamp()::text")
        .fetch_one(&pool)
        .await
        .expect("read the database clock before submitting");

    let first = deployment();
    assert_eq!(receipt(address, &first).await, created);
    assert_eq!(receipt(address, &first).await, unchanged);
    let mut second = first.clone();
    second["version"] = json!("v2");
    assert_eq!(receipt(address, &second).await, created);
    assert_eq!(receipt(address, &first).await, created);

    let mut explicit = first;
    explicit.as_object_mut().unwrap().remove("version");
    explicit["commit"] = json!(COMMIT);
    assert_eq!(receipt(address, &explicit).await, created);
    explicit["commit"] = json!(COMMIT.to_ascii_uppercase());
    assert_eq!(receipt(address, &explicit).await, unchanged);

    let history: Vec<(Option<String>, Option<String>)> = sqlx::query_as(
        "select version, commit_sha from deployment_events
         where sender = $1 and target_name = $2 and repository = $3 and auth_method = 'github_oidc'
           and received_at between $4::text::timestamptz and statement_timestamp()
         order by received_at, id",
    )
    .bind(SENDER)
    .bind(TARGET)
    .bind(SUBJECT)
    .bind(earliest)
    .fetch_all(&pool)
    .await
    .expect("read deployment history with provenance and receipt times");
    assert_eq!(
        history,
        [
            (Some("v1".into()), None),
            (Some("v2".into()), None),
            (Some("v1".into()), None),
            (None, Some(COMMIT.into())),
        ]
    );
    assert_eq!(event_count(&pool).await, 4);
}

// REQ-0052: malformed reports and targets outside the sender's registrations are rejected.
#[tokio::test]
async fn rejects_invalid_or_unregistered_deployments() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    register(address, SENDER, TARGET).await;

    for (field, value) in [
        ("target", json!("staging")),
        ("target", json!("production\n")),
        ("sender", json!("github.com/other/infra")),
        ("repository", json!("")),
        ("version", json!(null)),
        ("version", json!("x".repeat(257))),
        ("commit", json!(COMMIT)),
        ("auth_method", json!("unknown")),
        ("received_at", json!("2026-01-01T00:00:00Z")),
    ] {
        let mut report = deployment();
        report[field] = value;
        assert_eq!(
            submit(address, PATH, report.to_string()).await.0,
            400,
            "{field}"
        );
    }
    let mut report = deployment();
    report.as_object_mut().unwrap().remove("version");
    assert_eq!(submit(address, PATH, report.to_string()).await.0, 400);
    for invalid in ["abc123".to_owned(), "g".repeat(40), format!("{COMMIT}\n")] {
        report["commit"] = json!(invalid);
        assert_eq!(submit(address, PATH, report.to_string()).await.0, 400);
    }
    let duplicate = deployment()
        .to_string()
        .replacen('{', r#"{"target":"production","#, 1);
    assert_eq!(submit(address, PATH, duplicate).await.0, 400);
    assert_eq!(event_count(&pool).await, 0);
}

// REQ-0053: concurrent retries create one event per owner, target and subject.
#[tokio::test]
async fn scopes_current_deployments_and_serializes_concurrent_retries() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    register(address, SENDER, TARGET).await;
    register(address, SENDER, "staging").await;
    register(address, "deploy-key", TARGET).await;

    let report = deployment();
    let (first, second) = tokio::join!(receipt(address, &report), receipt(address, &report));
    let mut statuses = [first.0, second.0];
    statuses.sort_unstable();
    assert_eq!(statuses, [200, 201]);
    assert_eq!(event_count(&pool).await, 1);

    let mut another_subject = report.clone();
    another_subject["repository"] = json!("git.example.com/team/another");
    let mut another_target = report.clone();
    another_target["target"] = json!("staging");
    let mut another_owner = report;
    another_owner["sender"] = json!("deploy-key");
    another_owner["auth_method"] = json!("api_key");
    for report in [another_subject, another_target, another_owner] {
        assert_eq!(
            receipt(address, &report).await,
            (201, json!({"outcome": "created"}))
        );
    }
    assert_eq!(event_count(&pool).await, 4);
}
