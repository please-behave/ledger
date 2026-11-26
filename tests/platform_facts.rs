mod support;

use ledger::http;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use support::{migrated_pool, post, postgres};
use tokio::net::TcpListener;

const PATH: &str = "/v0/facts/platform-syncs";
const FIRST: &str = "2026-11-26T10:00:00Z";
const SECOND: &str = "2026-11-26T10:05:00Z";
const THIRD: &str = "2026-11-26T10:10:00Z";
const FOURTH: &str = "2026-11-26T10:15:00Z";
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER_COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn repository(name: &str) -> Value {
    json!({
        "name": name,
        "default_branch": "main",
        "archived": false,
        "fork": false,
        "last_activity_at": FIRST
    })
}

fn head(name: &str, commit: &str, observed_at: &str) -> Value {
    json!({
        "name": name,
        "branch": "main",
        "commit_sha": commit,
        "committed_at": "2026-11-25T00:00:00Z",
        "observed_at": observed_at
    })
}

fn sync(scope: &str, completed_at: &str, inventory: Vec<Value>, heads: Vec<Value>) -> Value {
    json!({
        "sender": "github-collector",
        "platform": "github",
        "host": "GitHub.COM",
        "scope": scope,
        "completed_at": completed_at,
        "inventory": inventory,
        "heads": heads
    })
}

async fn start_service(pool: PgPool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(http::run(listener, pool));
    address
}

async fn submit(address: SocketAddr, body: String) -> (u16, String) {
    tokio::task::spawn_blocking(move || post(address, PATH, &body))
        .await
        .expect("the request finishes")
}

async fn report(address: SocketAddr, snapshot: &Value) {
    let (status, body) = submit(address, snapshot.to_string()).await;
    assert_eq!(status, 204);
    assert!(body.is_empty());
}

async fn counts(pool: &PgPool) -> (i64, i64, i64) {
    sqlx::query_as(
        "select (select count(*) from repository_inventory),
                (select count(*) from repository_head_observations),
                (select count(*) from collector_heartbeats)",
    )
    .fetch_one(pool)
    .await
    .expect("count facts")
}

// REQ-0057, REQ-0058: retries preserve facts, while new snapshots reconcile inventory.
#[tokio::test]
async fn reconciles_inventory_and_records_each_head_observation_once() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;

    let first = sync(
        "Owner/*",
        FIRST,
        vec![repository("Name"), repository("Other")],
        vec![head("NAME", &COMMIT.to_ascii_uppercase(), FIRST)],
    );
    tokio::join!(report(address, &first), report(address, &first));
    assert_eq!(counts(&pool).await, (2, 1, 1));
    let original_receipt: String =
        sqlx::query_scalar("select received_at::text from repository_head_observations")
            .fetch_one(&pool)
            .await
            .expect("get the original receipt time");

    let mut changed = repository("name");
    changed["archived"] = json!(true);
    changed["fork"] = json!(true);
    changed["last_activity_at"] = json!(SECOND);
    let next = sync(
        "owner/*",
        SECOND,
        vec![changed.clone()],
        vec![head("name", OTHER_COMMIT, SECOND)],
    );
    report(address, &next).await;
    report(address, &next).await;
    report(address, &sync("owner/*", THIRD, vec![changed], vec![])).await;

    let attestation = json!({
        "sender": "ci",
        "attestation": {
            "schema_version": 1,
            "repository": "github.com/owner/name",
            "commit": COMMIT,
            "execution_points": ["pre-push"],
            "overall": "PASS",
            "results": [{"id": "format", "kind": "check", "status": "PASS"}]
        }
    });
    assert_eq!(
        tokio::task::spawn_blocking(move || {
            post(address, "/v0/attestations", &attestation.to_string())
        })
        .await
        .expect("the attestation submission finishes")
        .0,
        201
    );
    let inventory_matches: bool = sqlx::query_scalar(
        "select i.archived and i.fork and i.first_seen_at = $1::text::timestamptz
             and i.last_activity_at = $2::text::timestamptz
             and i.last_seen_at = $3::text::timestamptz and i.sender = 'github-collector'
         from repository_inventory i join attestations a using (repository)
         where i.platform = 'github'",
    )
    .bind(FIRST)
    .bind(SECOND)
    .bind(THIRD)
    .fetch_one(&pool)
    .await
    .expect("join inventory to evidence");
    assert!(inventory_matches);

    let omitted_once: bool = sqlx::query_scalar(
        "select gone_at = $1::text::timestamptz and gone_sender = 'github-collector'
         from repository_inventory where name = 'other'",
    )
    .bind(SECOND)
    .fetch_one(&pool)
    .await
    .expect("the omitted repository remains stored");
    assert!(omitted_once);

    let returned = sync(
        "owner/*",
        FOURTH,
        vec![repository("name"), repository("other")],
        vec![head("name", COMMIT, FOURTH)],
    );
    report(address, &returned).await;
    report(address, &next).await;
    let restored: bool = sqlx::query_scalar(
        "select gone_at is null and gone_sender is null and first_seen_at = $1::text::timestamptz
             and last_seen_at = $2::text::timestamptz
         from repository_inventory where name = 'other'",
    )
    .bind(FIRST)
    .bind(FOURTH)
    .fetch_one(&pool)
    .await
    .expect("read the restored repository");
    assert!(restored);

    let heartbeat_current: bool = sqlx::query_scalar(
        "select completed_at = $1::text::timestamptz and sender = 'github-collector'
             and platform = 'github' and host = 'github.com' and scope = 'owner/*'
         from collector_heartbeats",
    )
    .bind(FOURTH)
    .fetch_one(&pool)
    .await
    .expect("read the current heartbeat");
    assert!(heartbeat_current);
    let heads: Vec<(String, bool)> = sqlx::query_as(
        "select commit_sha, received_at::text = $1 from repository_head_observations
         where repository = 'github.com/owner/name' and branch = 'main'
           and committed_at = '2026-11-25T00:00:00Z' and sender = 'github-collector'
         order by observed_at",
    )
    .bind(original_receipt)
    .fetch_all(&pool)
    .await
    .expect("read head observations and their receipt times");
    assert_eq!(
        heads,
        [
            (COMMIT.into(), true),
            (OTHER_COMMIT.into(), false),
            (COMMIT.into(), false)
        ]
    );
    assert_eq!(counts(&pool).await, (2, 3, 1));
}

// REQ-0057, REQ-0058: a scope affects only the repositories it covers.
#[tokio::test]
async fn reconciles_overlapping_scopes_by_completion_time() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let initial = sync("owner/*", THIRD, vec![repository("name")], vec![]);
    let mut other_host = initial.clone();
    other_host["host"] = json!("git.example.org");
    report(address, &other_host).await;
    let mut other_platform = initial.clone();
    other_platform["platform"] = json!("another-platform");
    report(address, &other_platform).await;
    report(address, &initial).await;

    let old_head = head("late", COMMIT, FIRST);
    let late = sync(
        "owner/late",
        FIRST,
        vec![repository("late")],
        vec![old_head.clone()],
    );
    report(address, &late).await;
    let marked_gone: bool = sqlx::query_scalar(
        "select gone_at = $1::text::timestamptz from repository_inventory
         where platform = 'github' and host = 'github.com' and name = 'late'",
    )
    .bind(THIRD)
    .fetch_one(&pool)
    .await
    .expect("read the inventory introduced by a late report");
    assert!(marked_gone);

    let found = sync(
        "owner/late",
        FOURTH,
        vec![repository("late")],
        vec![old_head],
    );
    report(address, &found).await;
    report(address, &sync("owner/name", FOURTH, vec![], vec![])).await;
    let gone: Vec<(String, String, String)> = sqlx::query_as(
        "select platform, host, name from repository_inventory where gone_at is not null",
    )
    .fetch_all(&pool)
    .await
    .expect("read omissions");
    assert_eq!(
        gone,
        [("github".into(), "github.com".into(), "name".into())]
    );
    assert_eq!(counts(&pool).await, (4, 1, 5));
}

// REQ-0056: invalid payloads leave every fact untouched.
#[tokio::test]
async fn rejects_invalid_platform_syncs() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let mut entry = repository("name");
    entry["last_activity_at"] = Value::Null;
    let snapshot = sync(
        "owner/name",
        FIRST,
        vec![entry],
        vec![head("name", COMMIT, FIRST)],
    );
    let mut invalid = vec![
        ("/scope", json!("owner/*/name")),
        ("/scope", json!("owner/..")),
        ("/scope", json!("other/unknown")),
        ("/sender", json!("")),
        ("/host", json!("github.com:443")),
        ("/platform", json!("GitHub")),
        ("/completed_at", json!("2026-02-31T10:00:00Z")),
        ("/completed_at", json!("2026-11-26T10:00:00.0000001Z")),
        ("/inventory/0/fork", json!("false")),
        ("/inventory/0/name", json!("../name")),
        ("/heads/0/name", json!("unlisted")),
        ("/heads/0/branch", json!("other")),
        ("/heads/0/commit_sha", json!("abc123")),
        ("/heads/0/observed_at", json!(SECOND)),
    ];
    let mut duplicate_entry = repository("NAME");
    duplicate_entry["archived"] = json!(true);
    invalid.push(("/inventory", json!([repository("name"), duplicate_entry])));
    invalid.push((
        "/heads",
        json!([
            head("name", COMMIT, FIRST),
            head("NAME", OTHER_COMMIT, FIRST)
        ]),
    ));
    for (pointer, value) in invalid {
        let mut report = snapshot.clone();
        *report.pointer_mut(pointer).expect("field exists") = value;
        assert_eq!(
            submit(address, report.to_string()).await.0,
            400,
            "{pointer}"
        );
    }
    let duplicate_key = snapshot
        .to_string()
        .replacen('{', r#"{"sender":"other","#, 1);
    assert_eq!(submit(address, duplicate_key).await.0, 400);
    let mut unknown_field = snapshot.clone();
    unknown_field["heads"][0]["received_at"] = json!(FIRST);
    assert_eq!(submit(address, unknown_field.to_string()).await.0, 400);
    assert_eq!(counts(&pool).await, (0, 0, 0));
    report(address, &snapshot).await;
    assert_eq!(counts(&pool).await, (1, 1, 1));
}

// REQ-0056: a storage failure rolls back the entire sync and allows retry.
#[tokio::test]
async fn rolls_back_inventory_and_heartbeat_if_head_storage_fails() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    sqlx::query(
        "alter table repository_head_observations add constraint test_reject_head check (false)",
    )
    .execute(&pool)
    .await
    .expect("simulate a database error after inventory storage");
    let snapshot = sync(
        "owner/*",
        FIRST,
        vec![repository("name")],
        vec![head("name", COMMIT, FIRST)],
    );
    assert_eq!(submit(address, snapshot.to_string()).await.0, 500);
    assert_eq!(counts(&pool).await, (0, 0, 0));
    sqlx::query("alter table repository_head_observations drop constraint test_reject_head")
        .execute(&pool)
        .await
        .expect("allow head storage again");
    report(address, &snapshot).await;
    assert_eq!(counts(&pool).await, (1, 1, 1));
}
