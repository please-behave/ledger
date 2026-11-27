mod support;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use ledger::http;
use serde_json::Value;
use std::io::{self, Write};
use std::sync::{Arc, Mutex};
use support::{attestation_envelope, migrated_pool, postgres};
use tower::ServiceExt;

const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

#[derive(Clone, Default)]
struct LogBuffer(Arc<Mutex<Vec<u8>>>);

impl Write for LogBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

// REQ-0062: each completed read logs its outcome and timing without request data.
#[tokio::test]
async fn read_requests_log_timing_and_omit_request_data() {
    let postgres = postgres().await;
    let pool = migrated_pool(&postgres).await;
    let app = http::router(pool.clone());
    let logs = LogBuffer::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer({
            let logs = logs.clone();
            move || logs.clone()
        })
        .finish();
    // The separate test binary makes this the only subscriber for its tracing callsites.
    tracing::subscriber::set_global_default(subscriber).expect("install the test subscriber");

    let response = app
        .clone()
        .oneshot(
            Request::post("/v0/attestations")
                .body(Body::from(
                    attestation_envelope(
                        "github.com/example/private-body",
                        &["ci"],
                        COMMIT,
                        &["PASS"],
                    )
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let receipt: Value = serde_json::from_slice(&body).unwrap();
    let id = receipt["id"].as_str().unwrap();

    for (path_id, expected) in [
        (id, StatusCode::OK),
        ("private-path-id", StatusCode::BAD_REQUEST),
        (id, StatusCode::INTERNAL_SERVER_ERROR),
    ] {
        if expected == StatusCode::INTERNAL_SERVER_ERROR {
            pool.close().await;
        }
        let response = app
            .clone()
            .oneshot(
                Request::get(format!(
                    "/v0/attestations/{path_id}?private_query=private-value"
                ))
                .header("x-private-header", "private-header-value")
                .body(Body::from("private-body"))
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }

    let bytes = logs.0.lock().unwrap();
    let timing_lines: Vec<_> = std::str::from_utf8(&bytes)
        .unwrap()
        .lines()
        .map(|line| (line, serde_json::from_str::<Value>(line).unwrap()))
        .filter(|(_, entry)| entry["fields"]["message"] == "read request")
        .collect();
    assert_eq!(timing_lines.len(), 3);
    for ((line, entry), status) in timing_lines.iter().zip([200, 400, 500]) {
        assert_eq!(entry["level"], "INFO");
        let fields = &entry["fields"];
        assert_eq!(fields["route"], "/v0/attestations/{id}");
        assert_eq!(fields["status"], status);
        assert!(fields["duration_ms"].as_f64().unwrap() >= 0.0);
        for private in [
            id,
            "private-path-id",
            "private_query",
            "private-value",
            "x-private-header",
            "private-header-value",
            "private-body",
        ] {
            assert!(!line.contains(private), "{line}");
        }
    }
}
