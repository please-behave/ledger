use crate::attestation::Envelope;
use anyhow::{Context, Result};
use serde_json::Value;
use sqlx::PgPool;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use uuid::Uuid;

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

pub async fn append_attestation(
    pool: &PgPool,
    envelope: &Envelope,
    document: &Value,
) -> Result<Uuid, sqlx::Error> {
    let id = Uuid::now_v7();
    let attestation = &envelope.attestation;
    let mut transaction = pool.begin().await?;
    sqlx::query(
        "insert into attestations \
         (id, sender, repository, commit_sha, execution_points, overall, schema_version, document) \
         values ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(&envelope.sender)
    .bind(&attestation.repository)
    .bind(&attestation.commit)
    .bind(&attestation.execution_points)
    .bind(attestation.overall.as_str())
    .bind(attestation.schema_version)
    .bind(document)
    .execute(&mut *transaction)
    .await?;
    for (position, result) in (0_i32..).zip(&attestation.results) {
        sqlx::query(
            "insert into attestation_results (attestation_id, position, rule_id, kind, status) \
             values ($1, $2, $3, $4, $5)",
        )
        .bind(id)
        .bind(position)
        .bind(&result.id)
        .bind(result.kind.as_str())
        .bind(result.status.as_str())
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(id)
}
