mod support;

use ledger::http;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use support::{migrated_pool, post, postgres};
use tokio::net::TcpListener;
use uuid::Uuid;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn envelope() -> Value {
    json!({
        "sender": "ci",
        "attestation": {
            "schema_version": 1,
            "repository": "github.com/owner/name",
            "commit": COMMIT,
            "execution_points": ["pre-push", "ci"],
            "overall": "FAIL",
            "results": [
                {"id": "format", "kind": "check", "status": "PASS"},
                {"id": "review", "kind": "review", "status": "WARN"},
                {"id": "hooks", "kind": "integrity", "status": "NOT_RUN"}
            ]
        }
    })
}

async fn start_service(pool: PgPool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(http::run(listener, pool));
    address
}

async fn ingest(address: SocketAddr, body: String) -> (u16, String) {
    tokio::task::spawn_blocking(move || post(address, "/v0/attestations", &body))
        .await
        .expect("the request finishes")
}

async fn count(pool: &PgPool, table: &'static str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(format!("select count(*) from {table}")))
        .fetch_one(pool)
        .await
        .expect("count the rows")
}

async fn assert_nothing_stored(pool: &PgPool) {
    assert_eq!(count(pool, "attestations").await, 0);
    assert_eq!(count(pool, "attestation_results").await, 0);
}

fn id_of(body: &str) -> Uuid {
    let body: Value = serde_json::from_str(body).expect("a JSON body");
    body["id"]
        .as_str()
        .expect("the body names the id")
        .parse()
        .expect("the id is a UUID")
}

// REQ-0010: a valid envelope is appended with all of its results.
// REQ-0013: the received envelope is stored as received.
#[tokio::test(flavor = "multi_thread")]
async fn a_valid_envelope_is_stored_with_its_results() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;

    let (status, body) = ingest(address, envelope().to_string()).await;

    assert_eq!(status, 201);
    let id = id_of(&body);
    let row: (String, String, String, Vec<String>, String, i32, Value) = sqlx::query_as(
        "select sender, repository, commit_sha, execution_points, overall, schema_version, document \
         from attestations where id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("the attestation is stored");
    assert_eq!(row.0, "ci");
    assert_eq!(row.1, "github.com/owner/name");
    assert_eq!(row.2, COMMIT);
    assert_eq!(row.3, ["pre-push", "ci"]);
    assert_eq!(row.4, "FAIL");
    assert_eq!(row.5, 1);
    assert_eq!(row.6, envelope());
    let results: Vec<(i32, String, String, String)> = sqlx::query_as(
        "select position, rule_id, kind, status from attestation_results \
         where attestation_id = $1 order by position",
    )
    .bind(id)
    .fetch_all(&pool)
    .await
    .expect("the results are stored");
    let expected = [
        (0, "format", "check", "PASS"),
        (1, "review", "review", "WARN"),
        (2, "hooks", "integrity", "NOT_RUN"),
    ];
    assert_eq!(results.len(), expected.len());
    for (stored, expected) in results.iter().zip(expected) {
        assert_eq!(
            (
                stored.0,
                stored.1.as_str(),
                stored.2.as_str(),
                stored.3.as_str()
            ),
            expected
        );
    }
}

// REQ-0011: an invalid body is answered with 400 and leaves nothing behind.
#[tokio::test(flavor = "multi_thread")]
async fn an_invalid_structure_is_rejected_and_leaves_nothing_behind() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;

    let mut unknown_field = envelope();
    unknown_field["attestation"]["extra"] = json!(true);
    let mut unknown_result_field = envelope();
    unknown_result_field["attestation"]["results"][0]["extra"] = json!(true);
    let mut missing_field = envelope();
    missing_field["attestation"]
        .as_object_mut()
        .expect("an object")
        .remove("commit");
    let mut wrong_type = envelope();
    wrong_type["attestation"]["schema_version"] = json!("1");
    let mut unknown_status = envelope();
    unknown_status["attestation"]["results"][0]["status"] = json!("MAYBE");
    let mut unknown_overall = envelope();
    unknown_overall["attestation"]["overall"] = json!("NOT_RUN");
    let bodies = [
        "not json".to_string(),
        "[]".to_string(),
        unknown_field.to_string(),
        unknown_result_field.to_string(),
        missing_field.to_string(),
        wrong_type.to_string(),
        unknown_status.to_string(),
        unknown_overall.to_string(),
    ];

    for body in bodies {
        let (status, _) = ingest(address, body.clone()).await;
        assert_eq!(status, 400, "{body}");
    }
    assert_nothing_stored(&pool).await;
}

// REQ-0011: a violated database constraint is answered with 400.
#[tokio::test(flavor = "multi_thread")]
async fn a_violated_constraint_is_rejected_and_leaves_nothing_behind() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    let mut invalid_commit = envelope();
    invalid_commit["attestation"]["commit"] = json!("not-a-commit");

    let (status, _) = ingest(address, invalid_commit.to_string()).await;

    assert_eq!(status, 400);
    assert_nothing_stored(&pool).await;
}

// REQ-0012: nothing remains when storing the results fails.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_result_leaves_no_attestation_behind() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    sqlx::query(
        "alter table attestation_results add constraint reject_hooks check (rule_id <> 'hooks')",
    )
    .execute(&pool)
    .await
    .expect("add a constraint that rejects the last result");
    let address = start_service(pool.clone()).await;

    let (status, _) = ingest(address, envelope().to_string()).await;

    assert_eq!(status, 400);
    assert_nothing_stored(&pool).await;
}

// REQ-0014: identifiers are version 7 UUIDs that grow with every attestation.
#[tokio::test(flavor = "multi_thread")]
async fn identifiers_are_time_ordered() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool).await;

    let mut ids = Vec::new();
    for _ in 0..5 {
        let (status, body) = ingest(address, envelope().to_string()).await;
        assert_eq!(status, 201);
        ids.push(id_of(&body));
    }

    assert!(ids.iter().all(|id| id.get_version_num() == 7));
    assert!(ids.windows(2).all(|pair| pair[0] < pair[1]), "{ids:?}");
}
