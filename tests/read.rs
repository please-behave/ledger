mod support;

use ledger::http;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use support::{get, migrated_pool, post, postgres};
use tokio::net::TcpListener;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

fn envelope(repository: &str, points: &[&str], commit: &str, statuses: &[&str]) -> Value {
    let results: Vec<Value> = statuses
        .iter()
        .enumerate()
        .map(|(index, status)| json!({"id": format!("rule-{index}"), "kind": "check", "status": status}))
        .collect();
    json!({
        "sender": "ci",
        "attestation": {
            "schema_version": 1,
            "repository": repository,
            "commit": commit,
            "execution_points": points,
            "overall": "PASS",
            "results": results
        }
    })
}

fn commit(digit: char) -> String {
    digit.to_string().repeat(40)
}

async fn start_service(pool: PgPool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(http::run(listener, pool));
    address
}

async fn ingest(address: SocketAddr, envelope: Value) -> String {
    let body = envelope.to_string();
    let (status, body) =
        tokio::task::spawn_blocking(move || post(address, "/v0/attestations", &body))
            .await
            .expect("the request finishes");
    assert_eq!(status, 201);
    let body: Value = serde_json::from_str(&body).expect("a JSON body");
    body["id"].as_str().expect("the id").to_string()
}

async fn read(address: SocketAddr, path: &'static str) -> (u16, String) {
    tokio::task::spawn_blocking(move || get(address, path))
        .await
        .expect("the request finishes")
}

async fn read_json(address: SocketAddr, path: String) -> (u16, Value) {
    let (status, body) = tokio::task::spawn_blocking(move || get(address, &path))
        .await
        .expect("the request finishes");
    (status, serde_json::from_str(&body).unwrap_or(Value::Null))
}

async fn latest(address: SocketAddr) -> Vec<Value> {
    let (status, body) = read_json(address, "/v0/attestations/latest".to_string()).await;
    assert_eq!(status, 200);
    body.as_array().expect("an array").clone()
}

// REQ-0015: the newest attestation of every repository and execution points, in a stable order.
#[tokio::test(flavor = "multi_thread")]
async fn latest_selects_the_newest_attestation_of_every_combination_in_order() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool).await;
    let (b, a) = ("github.com/o/b", "github.com/o/a");
    ingest(address, envelope(b, &["ci"], &commit('1'), &[])).await;
    ingest(address, envelope(a, &["pre-push", "ci"], &commit('2'), &[])).await;
    ingest(address, envelope(a, &["pre-push"], &commit('3'), &[])).await;
    ingest(address, envelope(a, &["ci"], &commit('4'), &[])).await;
    let newest_b = ingest(address, envelope(b, &["ci"], &commit('5'), &[])).await;
    let newest_a = ingest(address, envelope(a, &["pre-push"], &commit('6'), &[])).await;

    let summaries = latest(address).await;

    let order: Vec<(&str, Vec<&str>, &str)> = summaries
        .iter()
        .map(|summary| {
            (
                summary["repository"].as_str().expect("repository"),
                summary["execution_points"]
                    .as_array()
                    .expect("points")
                    .iter()
                    .map(|point| point.as_str().expect("point"))
                    .collect(),
                summary["commit"].as_str().expect("commit"),
            )
        })
        .collect();
    let (commit4, commit2, commit6, commit5) = (commit('4'), commit('2'), commit('6'), commit('5'));
    assert_eq!(
        order,
        [
            (a, vec!["ci"], commit4.as_str()),
            (a, vec!["pre-push"], commit6.as_str()),
            (a, vec!["pre-push", "ci"], commit2.as_str()),
            (b, vec!["ci"], commit5.as_str()),
        ]
    );
    assert_eq!(summaries[1]["id"], newest_a);
    assert_eq!(summaries[3]["id"], newest_b);
}

// REQ-0015: nothing stored is an empty list.
#[tokio::test(flavor = "multi_thread")]
async fn latest_is_empty_without_attestations() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;

    assert!(latest(address).await.is_empty());
}

// REQ-0016: the summary counts WARN and NOT_RUN results.
// REQ-0020: the summary counts all results.
// REQ-0017: the attestation has the summary fields and its results in stored order.
#[tokio::test(flavor = "multi_thread")]
async fn counts_and_results_follow_the_stored_attestation() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    let statuses = ["WARN", "PASS", "NOT_RUN", "WARN", "FAIL", "NOT_RUN", "WARN"];
    let id = ingest(address, envelope("r", &["ci"], COMMIT, &statuses)).await;

    let summaries = latest(address).await;
    let (status, attestation) = read_json(address, format!("/v0/attestations/{id}")).await;

    assert_eq!(summaries[0]["warn_count"], 3);
    assert_eq!(summaries[0]["not_run_count"], 2);
    assert_eq!(summaries[0]["result_count"], 7);
    assert_eq!(status, 200);
    assert_eq!(attestation["id"], id);
    assert_eq!(attestation["sender"], "ci");
    assert_eq!(attestation["repository"], "r");
    assert_eq!(attestation["commit"], COMMIT);
    assert_eq!(attestation["execution_points"], json!(["ci"]));
    assert_eq!(attestation["overall"], "PASS");
    assert_eq!(attestation["warn_count"], 3);
    assert_eq!(attestation["not_run_count"], 2);
    assert_eq!(attestation["result_count"], 7);
    assert!(
        attestation["received_at"]
            .as_str()
            .is_some_and(|at| at.ends_with('Z'))
    );
    let results: Vec<(&str, &str)> = attestation["results"]
        .as_array()
        .expect("results")
        .iter()
        .map(|result| {
            (
                result["id"].as_str().expect("id"),
                result["status"].as_str().expect("status"),
            )
        })
        .collect();
    let expected: Vec<(String, &str)> = statuses
        .iter()
        .enumerate()
        .map(|(index, status)| (format!("rule-{index}"), *status))
        .collect();
    assert_eq!(
        results,
        expected
            .iter()
            .map(|(id, status)| (id.as_str(), *status))
            .collect::<Vec<_>>()
    );
    assert_eq!(attestation["results"][0]["kind"], "check");
}

// REQ-0016, REQ-0020: an attestation without results has zero counts and an empty result list.
// REQ-0017: its results are an empty list.
#[tokio::test(flavor = "multi_thread")]
async fn an_attestation_without_results_has_zero_counts() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    let id = ingest(address, envelope("r", &["ci"], COMMIT, &[])).await;

    let summaries = latest(address).await;
    let (status, attestation) = read_json(address, format!("/v0/attestations/{id}")).await;

    assert_eq!(summaries[0]["warn_count"], 0);
    assert_eq!(summaries[0]["not_run_count"], 0);
    assert_eq!(summaries[0]["result_count"], 0);
    assert_eq!(status, 200);
    assert_eq!(attestation["result_count"], 0);
    assert_eq!(attestation["results"], json!([]));
}

// REQ-0018: an unknown identifier is 404 and a malformed one is 400.
#[tokio::test(flavor = "multi_thread")]
async fn unknown_and_malformed_identifiers_are_told_apart() {
    let postgres = postgres().await;
    let address = start_service(migrated_pool(&postgres).await).await;
    ingest(address, envelope("r", &["ci"], COMMIT, &[])).await;

    let (unknown, _) = read(
        address,
        "/v0/attestations/0190a8c2-0000-7000-8000-000000000000",
    )
    .await;
    let (malformed, _) = read(address, "/v0/attestations/not-a-uuid").await;

    assert_eq!(unknown, 404);
    assert_eq!(malformed, 400);
}

// REQ-0019: reading leaves the stored data as it was.
#[tokio::test(flavor = "multi_thread")]
async fn reading_changes_nothing() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let address = start_service(pool.clone()).await;
    let id = ingest(address, envelope("r", &["ci"], COMMIT, &["WARN"])).await;
    let snapshot = || async {
        let attestations: Vec<String> =
            sqlx::query_scalar("select attestations::text from attestations order by id")
                .fetch_all(&pool)
                .await
                .expect("read the attestations");
        let results: Vec<String> = sqlx::query_scalar(
            "select attestation_results::text from attestation_results order by attestation_id, position",
        )
        .fetch_all(&pool)
        .await
        .expect("read the results");
        (attestations, results)
    };
    let before = snapshot().await;

    latest(address).await;
    read_json(address, format!("/v0/attestations/{id}")).await;
    read(address, "/v0/attestations/not-a-uuid").await;

    assert_eq!(snapshot().await, before);
}
