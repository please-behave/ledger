mod support;

use ledger::http;
use serde::Deserialize;
use serde_json::value::RawValue;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use support::{migrated_pool, post, postgres};
use tokio::net::TcpListener;

const ROUTE: &str = "/v0/rule-registrations";
const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";
const RETRY_COMMIT: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

#[derive(Deserialize)]
struct Vector {
    input: Box<RawValue>,
    digest: String,
}

fn vectors() -> Vec<Vector> {
    serde_json::from_str(include_str!("../why/contracts/rule-digest/vectors.json")).unwrap()
}

async fn start_service(pool: PgPool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let address = listener.local_addr().expect("address");
    tokio::spawn(http::run(listener, pool));
    address
}

async fn submit(address: SocketAddr, body: String) -> (u16, String) {
    tokio::task::spawn_blocking(move || post(address, ROUTE, &body))
        .await
        .expect("request finishes")
}

async fn outcomes(address: SocketAddr, body: String) -> Value {
    let (status, receipt) = submit(address, body).await;
    assert_eq!(status, 200, "{receipt}");
    serde_json::from_str::<Value>(&receipt).expect("JSON receipt")["outcomes"].clone()
}

fn batch(sender: &str, commit: &str, rules: &str) -> String {
    format!(r#"{{"sender":"{sender}","commit":"{commit}","rules":[{rules}]}}"#)
}

async fn stored_review(pool: &PgPool) -> Value {
    sqlx::query_scalar(
        "select jsonb_build_object( \
          'name', name, 'description', description, 'blocking', blocking, 'prompt', prompt, \
          'knowledge', knowledge::text, 'digest', digest, 'sender', sender, \
          'commit', commit_sha, 'registered_at', registered_at) \
         from rule_registrations where id = 'summary' and version = '2.10'",
    )
    .fetch_one(pool)
    .await
    .expect("the original review registration")
}

// REQ-0023, REQ-0026
#[tokio::test(flavor = "multi_thread")]
async fn registrations_match_vectors_and_preserve_the_original() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let vectors = vectors();
    let rules = vectors
        .iter()
        .map(|vector| vector.input.get())
        .collect::<Vec<_>>()
        .join(",");
    let first = outcomes(address, batch("ci", COMMIT, &rules)).await;
    assert_eq!(
        first,
        json!([
            {"outcome": "created", "digest": vectors[0].digest},
            {"outcome": "created", "digest": vectors[1].digest},
            {"outcome": "unchanged", "digest": vectors[2].digest},
        ])
    );

    #[derive(Deserialize)]
    struct ReviewInput {
        name: String,
        description: String,
        prompt: String,
        knowledge: Box<RawValue>,
    }
    let review: ReviewInput = serde_json::from_str(vectors[1].input.get()).unwrap();
    let original = stored_review(&pool).await;
    assert_eq!(original["name"], review.name);
    assert_eq!(original["description"], review.description);
    assert_eq!(original["prompt"], review.prompt);
    assert_eq!(original["blocking"], false);
    assert_eq!(original["sender"], "ci");
    assert_eq!(original["commit"], COMMIT);
    assert_eq!(original["digest"], vectors[1].digest);
    assert_eq!(original["knowledge"], review.knowledge.get());
    assert!(original["registered_at"].is_string());

    let retry = outcomes(address, batch("another-ci", RETRY_COMMIT, &rules)).await;
    assert_eq!(
        retry,
        json!([
            {"outcome": "unchanged", "digest": vectors[0].digest},
            {"outcome": "unchanged", "digest": vectors[1].digest},
            {"outcome": "unchanged", "digest": vectors[2].digest},
        ])
    );

    let mut changed: Value = serde_json::from_str(vectors[1].input.get()).unwrap();
    changed["name"] = json!("New display name");
    let mut new_version = changed.clone();
    new_version["version"] = json!("2.11");
    let mixed = outcomes(
        address,
        batch(
            "another-ci",
            RETRY_COMMIT,
            &format!("{changed},{new_version}"),
        ),
    )
    .await;
    assert_eq!(mixed[0]["outcome"], "conflict");
    assert!(
        mixed[0]["digest"]
            .as_str()
            .unwrap()
            .starts_with("v1:sha256:")
    );
    assert_ne!(mixed[0]["digest"], vectors[1].digest);
    assert_eq!(mixed[1]["outcome"], "created");
    assert_eq!(stored_review(&pool).await, original);

    let stored_digest: String = sqlx::query_scalar(
        "select digest from rule_registrations where id = 'summary' and version = '2.11'",
    )
    .fetch_one(&pool)
    .await
    .expect("the next version exists alongside the original");
    assert_eq!(mixed[1]["digest"], stored_digest);
}

// REQ-0026
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_repetitions_create_one_registration() {
    let database = postgres().await;
    let address = start_service(migrated_pool(&database).await).await;
    let body = batch("ci", COMMIT, vectors()[0].input.get());
    let (first, second) = tokio::join!(outcomes(address, body.clone()), outcomes(address, body),);
    let mut statuses = [
        first[0]["outcome"].as_str().unwrap(),
        second[0]["outcome"].as_str().unwrap(),
    ];
    statuses.sort_unstable();
    assert_eq!(statuses, ["created", "unchanged"]);
    assert_eq!(first[0]["digest"], second[0]["digest"]);
}

// REQ-0027
#[tokio::test(flavor = "multi_thread")]
async fn invalid_batches_leave_the_registry_empty() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let cases = vectors();
    let check = cases[0].input.get();
    let review = cases[1].input.get();
    let positional_rule = json!([
        "github.com/example/catalog",
        ".please/checks/format",
        "format",
        "1.0",
        "check"
    ]);
    let positional_envelope = format!(r#"["ci","{COMMIT}",[{check}]]"#);
    assert_eq!(submit(address, positional_envelope).await.0, 400);
    assert_eq!(
        submit(address, batch("ci", COMMIT, &positional_rule.to_string()))
            .await
            .0,
        400
    );
    for (label, invalid_rule) in [
        ("version syntax", check.replace("1.0", "01.0")),
        (
            "version length",
            check.replace("1.0", &format!("1.{}", "2".repeat(63))),
        ),
        (
            "identity length",
            check.replace("github.com/example/catalog", &"📝".repeat(257)),
        ),
        ("unknown kind", check.replace("\"check\"", "\"integrity\"")),
        (
            "review-only fields",
            check.replace("\"check\"", "\"check\", \"blocking\": false"),
        ),
        (
            "missing blocking",
            review.replace("\"blocking\": false,", ""),
        ),
        (
            "null optional field",
            check.replace("\"check\"", "\"check\", \"name\": null"),
        ),
        (
            "duplicate rule field",
            check.replace("\"check\"", "\"check\", \"kind\": \"check\""),
        ),
        (
            "duplicate nested knowledge field",
            check.replace(
                "\"check\"",
                r#""check", "knowledge": {"items":[{"a": true, "\u0061": false}]}"#,
            ),
        ),
        (
            "normalized knowledge key collision",
            check.replace(
                "\"check\"",
                r#""check", "knowledge": {"\r\n": true, "\n": false}"#,
            ),
        ),
        (
            "invalid knowledge value",
            check.replace("\"check\"", r#""check", "knowledge": {"count": 1}"#),
        ),
    ] {
        let (status, _) = submit(
            address,
            batch("ci", COMMIT, &format!("{check},{invalid_rule}")),
        )
        .await;
        assert_eq!(status, 400, "{label}");
    }
    assert_eq!(submit(address, batch("ci", "invalid", check)).await.0, 400);
    assert_eq!(submit(address, batch("", COMMIT, check)).await.0, 400);
    assert_eq!(submit(address, batch("ci", COMMIT, "")).await.0, 400);
    let too_many = std::iter::repeat_n(check, 101)
        .collect::<Vec<_>>()
        .join(",");
    assert_eq!(submit(address, batch("ci", COMMIT, &too_many)).await.0, 400);
    let mut too_large = batch("ci", COMMIT, check);
    too_large.push_str(&" ".repeat(2 * 1024 * 1024));
    assert_eq!(submit(address, too_large).await.0, 413);

    let rows: i64 = sqlx::query_scalar("select count(*) from rule_registrations")
        .fetch_one(&pool)
        .await
        .expect("count registrations");
    assert_eq!(rows, 0);
}
