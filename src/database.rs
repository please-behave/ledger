use anyhow::{Context, Result};
use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;

static MIGRATOR: Migrator = sqlx::migrate!();

const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(2);

pub async fn connect(url: &str) -> Result<PgPool> {
    pool_options()
        .connect(url)
        .await
        .context("cannot connect to the database")
}

pub fn connect_lazily(url: &str) -> Result<PgPool> {
    pool_options()
        .connect_lazy(url)
        .context("the database URL is invalid")
}

fn pool_options() -> PgPoolOptions {
    PgPoolOptions::new().acquire_timeout(ACQUIRE_TIMEOUT)
}

pub async fn migrate(pool: &PgPool) -> Result<()> {
    MIGRATOR
        .run(pool)
        .await
        .context("cannot apply the migrations")
}

pub async fn schema_ready(pool: &PgPool) -> bool {
    let applied: Result<Vec<i64>, _> =
        sqlx::query_scalar("select version from _sqlx_migrations where success")
            .fetch_all(pool)
            .await;
    match applied {
        Ok(applied) => MIGRATOR
            .iter()
            .all(|migration| applied.contains(&migration.version)),
        Err(error) => {
            tracing::warn!(%error, "the schema is not ready");
            false
        }
    }
}
