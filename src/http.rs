use crate::attestation::Envelope;
use crate::database;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router, serve};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::signal;

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/v0/attestations", post(ingest))
        .with_state(pool)
}

pub async fn run(listener: TcpListener, pool: PgPool) -> std::io::Result<()> {
    serve(listener, router(pool))
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn healthz() -> StatusCode {
    StatusCode::OK
}

async fn readyz(State(pool): State<PgPool>) -> StatusCode {
    if database::schema_ready(&pool).await {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn ingest(
    State(pool): State<PgPool>,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let document: Value = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let envelope = Envelope::deserialize(&document).map_err(|_| StatusCode::BAD_REQUEST)?;
    match database::append_attestation(&pool, &envelope, &document).await {
        Ok(id) => Ok((StatusCode::CREATED, Json(json!({ "id": id.to_string() })))),
        Err(error) => Err(storage_failure(&error)),
    }
}

fn storage_failure(error: &sqlx::Error) -> StatusCode {
    let violates_constraint = error
        .as_database_error()
        .and_then(|error| error.code())
        .is_some_and(|code| code.starts_with("23"));
    if violates_constraint {
        StatusCode::BAD_REQUEST
    } else {
        tracing::error!(%error, "cannot store the attestation");
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

async fn shutdown_signal() {
    let interrupt = async {
        let _ = signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut stream) = signal::unix::signal(signal::unix::SignalKind::terminate()) {
            stream.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
}
