use crate::database;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Router, serve};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tokio::signal;

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
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
