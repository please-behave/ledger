mod support;

use ledger::http;
use serde_json::{Value, json};
use sqlx::PgPool;
use std::net::SocketAddr;
use support::{migrated_pool, post, postgres};
use tokio::net::TcpListener;

const PATH: &str = "/v0/target-registrations";
const OWNER: &str = "github.com/owner/project";

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

async fn receipt(address: SocketAddr, request: &Value) -> Value {
    let (status, body) = submit(address, request.to_string()).await;
    assert_eq!(status, 200, "{body}");
    serde_json::from_str(&body).expect("a JSON receipt")
}

// REQ-0051: targets are owner-scoped and retries preserve the first registration.
#[tokio::test]
async fn registers_targets_in_request_order_and_preserves_the_first_description() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let description = "é".repeat(512);
    let batch = json!({
        "sender": OWNER,
        "targets": [
            {"name": "production.eu-west_1", "description": description},
            {"name": "dev"},
            {"name": "production.eu-west_1", "description": "Later in the batch"}
        ]
    });
    assert_eq!(
        receipt(address, &batch).await,
        json!({"outcomes": [
            {"name": "production.eu-west_1", "outcome": "created"},
            {"name": "dev", "outcome": "created"},
            {"name": "production.eu-west_1", "outcome": "unchanged"}
        ]})
    );

    let mut retry = json!({
        "sender": OWNER,
        "targets": [{"name": "production.eu-west_1", "description": "Retry"}]
    });
    assert_eq!(
        receipt(address, &retry).await,
        json!({"outcomes": [{"name": "production.eu-west_1", "outcome": "unchanged"}]})
    );
    retry["sender"] = json!("deployment CI");
    assert_eq!(
        receipt(address, &retry).await,
        json!({"outcomes": [{"name": "production.eu-west_1", "outcome": "created"}]})
    );

    let stored: Vec<(String, String, Option<String>)> = sqlx::query_as(
        "select owner, name, description from deployment_targets order by owner, name",
    )
    .fetch_all(&pool)
    .await
    .expect("read the targets");
    assert_eq!(
        stored,
        [
            (
                "deployment CI".into(),
                "production.eu-west_1".into(),
                Some("Retry".into())
            ),
            (OWNER.into(), "dev".into(), None),
            (
                OWNER.into(),
                "production.eu-west_1".into(),
                Some(description)
            ),
        ]
    );
}

// REQ-0050: invalid or oversized batches create no targets.
#[tokio::test]
async fn rejects_an_entire_batch_when_any_entry_is_invalid() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let valid = json!({"name": "production"});
    for invalid in [
        json!({"name": ""}),
        json!({"name": "-prod"}),
        json!({"name": "prod west"}),
        json!({"name": "prod\n"}),
        json!({"name": "a".repeat(65)}),
        json!({"name": "dev", "description": ""}),
        json!({"name": "dev", "description": "é".repeat(513)}),
        json!({"name": "dev", "description": "a\0b"}),
        json!({"name": "dev", "description": null}),
        json!({"name": "dev", "owner": "spoofed"}),
    ] {
        let batch = json!({"sender": OWNER, "targets": [valid, invalid]});
        assert_eq!(submit(address, batch.to_string()).await.0, 400, "{batch}");
    }
    for invalid in [
        json!({"targets": [valid]}),
        json!({"sender": "", "targets": [valid]}),
        json!({"sender": "a\0b", "targets": [valid]}),
        json!({"sender": "é".repeat(257), "targets": [valid]}),
        json!({"sender": OWNER, "targets": []}),
        json!({"sender": OWNER, "targets": vec![valid.clone(); 101]}),
    ] {
        assert_eq!(
            submit(address, invalid.to_string()).await.0,
            400,
            "{invalid}"
        );
    }
    let duplicate_key = r#"{"sender":"ci","targets":[{"name":"one","name":"two"}]}"#;
    assert_eq!(submit(address, duplicate_key.into()).await.0, 400);

    let mut oversized = json!({"sender": OWNER, "targets": [valid]}).to_string();
    oversized.push_str(&" ".repeat(2 * 1024 * 1024));
    assert_eq!(submit(address, oversized).await.0, 413);

    let count: i64 = sqlx::query_scalar("select count(*) from deployment_targets")
        .fetch_one(&pool)
        .await
        .expect("count targets");
    assert_eq!(count, 0);
}

// REQ-0051: overlapping concurrent batches agree on the winning registration.
#[tokio::test]
async fn concurrent_batches_create_each_target_once() {
    let database = postgres().await;
    let pool = migrated_pool(&database).await;
    let address = start_service(pool.clone()).await;
    let first = json!({"sender": OWNER, "targets": [
        {"name": "dev", "description": "First"},
        {"name": "prod", "description": "First"}
    ]});
    let second = json!({"sender": OWNER, "targets": [
        {"name": "prod", "description": "Second"},
        {"name": "dev", "description": "Second"}
    ]});

    let (first_receipt, second_receipt) =
        tokio::join!(receipt(address, &first), receipt(address, &second));
    let (first_outcome, second_outcome, description) =
        if first_receipt["outcomes"][0]["outcome"] == "created" {
            ("created", "unchanged", "First")
        } else {
            ("unchanged", "created", "Second")
        };
    assert_eq!(
        first_receipt,
        json!({"outcomes": [
            {"name": "dev", "outcome": first_outcome},
            {"name": "prod", "outcome": first_outcome}
        ]})
    );
    assert_eq!(
        second_receipt,
        json!({"outcomes": [
            {"name": "prod", "outcome": second_outcome},
            {"name": "dev", "outcome": second_outcome}
        ]})
    );
    let descriptions: Vec<String> =
        sqlx::query_scalar("select description from deployment_targets")
            .fetch_all(&pool)
            .await
            .expect("read the descriptions");
    assert_eq!(descriptions, [description, description]);
}
