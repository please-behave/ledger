use crate::attestation::Envelope;
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::{FromRow, PgPool};
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
        let rule = result.rule.as_ref();
        sqlx::query(
            "insert into attestation_results \
             (attestation_id, position, rule_id, kind, status, rule_source, rule_path, \
              rule_local_id, rule_version, rule_digest) \
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(id)
        .bind(position)
        .bind(&result.id)
        .bind(result.kind.as_str())
        .bind(result.status.as_str())
        .bind(rule.map(|rule| rule.source.as_str()))
        .bind(rule.map(|rule| rule.path.as_str()))
        .bind(rule.map(|rule| rule.id.as_str()))
        .bind(rule.and_then(|rule| rule.version.as_deref()))
        .bind(rule.and_then(|rule| rule.digest.as_deref()))
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(id)
}

#[derive(Serialize, FromRow)]
pub struct Summary {
    id: String,
    received_at: String,
    sender: String,
    repository: String,
    commit: String,
    execution_points: Vec<String>,
    overall: String,
    warn_count: i64,
    not_run_count: i64,
    result_count: i64,
}

#[derive(Serialize, FromRow)]
pub struct ResultRow {
    id: String,
    kind: String,
    status: String,
}

#[derive(Serialize)]
pub struct StoredAttestation {
    #[serde(flatten)]
    summary: Summary,
    results: Vec<ResultRow>,
}

macro_rules! summary_query {
    ($source:literal) => {
        concat!(
            "select a.id::text as id, \
             to_char(a.received_at at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') as received_at, \
             a.sender, a.repository, a.commit_sha as \"commit\", a.execution_points, a.overall, \
             (select count(*) from attestation_results r \
              where r.attestation_id = a.id and r.status = 'WARN') as warn_count, \
             (select count(*) from attestation_results r \
              where r.attestation_id = a.id and r.status = 'NOT_RUN') as not_run_count, \
             (select count(*) from attestation_results r \
              where r.attestation_id = a.id) as result_count \
             from ",
            $source
        )
    };
}

pub async fn latest_attestations(pool: &PgPool) -> Result<Vec<Summary>, sqlx::Error> {
    sqlx::query_as(summary_query!(
        "(select distinct on (repository, execution_points) * from attestations \
          order by repository, execution_points, received_at desc, id desc) a \
         order by a.repository, a.execution_points"
    ))
    .fetch_all(pool)
    .await
}

pub async fn attestation_history(
    pool: &PgPool,
    before: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Summary>, sqlx::Error> {
    sqlx::query_as(summary_query!(
        "(select * from attestations where ($1::uuid is null or id < $1) \
          order by id desc limit $2) a \
         order by a.id desc"
    ))
    .bind(before)
    .bind(limit)
    .fetch_all(pool)
    .await
}

pub async fn attestation(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<StoredAttestation>, sqlx::Error> {
    let summary: Option<Summary> = sqlx::query_as(summary_query!("attestations a where a.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let Some(summary) = summary else {
        return Ok(None);
    };
    let results = sqlx::query_as(
        "select rule_id as id, kind, status from attestation_results \
         where attestation_id = $1 order by position",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(Some(StoredAttestation { summary, results }))
}
