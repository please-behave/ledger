mod support;

use ledger::http;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use support::{migrated_pool, post, postgres};
use tokio::net::TcpListener;

const PATH: &str = "/v0/facts/releases";
const REPOSITORY: &str = "github.com/owner/project";
const VERSION: &str = " November release ";
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_COMMIT: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn release_fact() -> Value {
    json!({
        "repository": REPOSITORY,
        "version": VERSION,
        "commit": COMMIT,
        "sender": REPOSITORY,
        "auth_method": "github_oidc"
    })
}

async fn start_service(pool: PgPool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(http::run(listener, pool));
    address
}

async fn ingest(address: SocketAddr, body: String) -> (u16, String) {
    tokio::task::spawn_blocking(move || post(address, PATH, &body))
        .await
        .expect("the request finishes")
}

async fn receipt(address: SocketAddr, fact: &Value) -> (u16, Value) {
    let (status, body) = ingest(address, fact.to_string()).await;
    (status, serde_json::from_str(&body).expect("a JSON receipt"))
}

async fn stored_fact(pool: &PgPool, repository: &str) -> (String, String, String, String) {
    sqlx::query_as(
        "select commit_sha, sender, auth_method, received_at::text from release_facts \
         where repository = $1 and version = $2",
    )
    .bind(repository)
    .bind(VERSION)
    .fetch_one(pool)
    .await
    .expect("read the stored release fact")
}

// REQ-0044: the first fact, its receipt time and its provenance survive retries and conflicts.
#[tokio::test]
async fn records_the_first_fact_for_each_repository_and_version() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let fact = release_fact();

    assert_eq!(
        receipt(address, &fact).await,
        (201, json!({"outcome": "created"}))
    );
    let first = stored_fact(&pool, REPOSITORY).await;
    assert_eq!(first.0, COMMIT);
    assert_eq!(first.1, REPOSITORY);
    assert_eq!(first.2, "github_oidc");
    assert_eq!(
        receipt(address, &fact).await,
        (200, json!({"outcome": "unchanged"}))
    );

    let mut retry = fact;
    retry["commit"] = json!(COMMIT.to_ascii_uppercase());
    retry["sender"] = json!("release workflow");
    retry["auth_method"] = json!("api_key");
    assert_eq!(
        receipt(address, &retry).await,
        (200, json!({"outcome": "unchanged"}))
    );
    retry["commit"] = json!(OTHER_COMMIT);
    assert_eq!(
        receipt(address, &retry).await,
        (409, json!({"outcome": "conflict"}))
    );
    assert_eq!(stored_fact(&pool, REPOSITORY).await, first);

    let other_repository = "git.example.com/team/project";
    retry["repository"] = json!(other_repository);
    assert_eq!(
        receipt(address, &retry).await,
        (201, json!({"outcome": "created"}))
    );
    let other = stored_fact(&pool, other_repository).await;
    assert_eq!(other.0, OTHER_COMMIT);
    assert_eq!(other.1, "release workflow");
    assert_eq!(other.2, "api_key");

    let attestations: i64 = sqlx::query_scalar("select count(*) from attestations")
        .fetch_one(&pool)
        .await
        .expect("count attestations");
    assert_eq!(attestations, 0);
}

// REQ-0043: invalid and mismatched sender submissions leave storage empty.
#[tokio::test]
async fn rejects_invalid_release_facts() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;

    for (field, value) in [
        ("commit", json!("0123456")),
        ("commit", json!("g".repeat(40))),
        ("commit", json!(format!("{COMMIT}\n"))),
        ("version", json!("")),
        ("version", json!("🛠".repeat(257))),
        ("version", json!("a\0b")),
        ("repository", json!("")),
        ("repository", json!("github.com/owner/other")),
        ("sender", json!("")),
        ("auth_method", json!("anonymous")),
        ("received_at", json!("2026-01-01T00:00:00Z")),
    ] {
        let mut invalid = release_fact();
        invalid[field] = value;
        let (status, _) = ingest(address, invalid.to_string()).await;
        assert_eq!(status, 400, "invalid {field}");
    }
    for invalid in [
        "[]".to_owned(),
        release_fact()
            .to_string()
            .replacen('{', r#"{"version":"duplicate","#, 1),
    ] {
        assert_eq!(ingest(address, invalid).await.0, 400);
    }

    let count: i64 = sqlx::query_scalar("select count(*) from release_facts")
        .fetch_one(&pool)
        .await
        .expect("count release facts");
    assert_eq!(count, 0);
}

// REQ-0044: concurrent submissions for one version agree on the winner.
#[tokio::test]
async fn concurrent_commits_create_one_fact_and_report_one_conflict() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let first = release_fact();
    let mut second = release_fact();
    second["commit"] = json!(OTHER_COMMIT);

    let (first_receipt, second_receipt) =
        tokio::join!(receipt(address, &first), receipt(address, &second));
    let (winner, loser, commit) = if first_receipt.0 == 201 {
        (first_receipt, second_receipt, COMMIT)
    } else {
        (second_receipt, first_receipt, OTHER_COMMIT)
    };
    assert_eq!(winner, (201, json!({"outcome": "created"})));
    assert_eq!(loser, (409, json!({"outcome": "conflict"})));
    assert_eq!(stored_fact(&pool, REPOSITORY).await.0, commit);
}
