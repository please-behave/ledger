use crate::attestation::Envelope;
use crate::registration::{OutcomeKind, Rule};
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

pub(crate) async fn register_rule(
    pool: &PgPool,
    sender: &str,
    commit: &str,
    rule: &Rule,
    digest: &str,
) -> Result<OutcomeKind, sqlx::Error> {
    let inserted = sqlx::query(
        "insert into rule_registrations \
         (source, path, id, version, kind, name, description, blocking, prompt, knowledge, \
          digest, sender, commit_sha) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10::text::json, $11, $12, $13) \
         on conflict (source, path, id, version) do nothing",
    )
    .bind(&rule.source)
    .bind(&rule.path)
    .bind(&rule.id)
    .bind(&rule.version)
    .bind(rule.kind.as_str())
    .bind(&rule.name)
    .bind(&rule.description)
    .bind(rule.blocking)
    .bind(&rule.prompt)
    .bind(
        rule.knowledge
            .as_deref()
            .map(serde_json::value::RawValue::get),
    )
    .bind(digest)
    .bind(sender)
    .bind(commit)
    .execute(pool)
    .await?;
    if inserted.rows_affected() == 1 {
        return Ok(OutcomeKind::Created);
    }

    // The insert may have waited for a concurrent writer. A new statement
    // can see the committed row that won the key, without updating it.
    let stored_digest: String = sqlx::query_scalar(
        "select digest from rule_registrations \
         where source = $1 and path = $2 and id = $3 and version = $4",
    )
    .bind(&rule.source)
    .bind(&rule.path)
    .bind(&rule.id)
    .bind(&rule.version)
    .fetch_one(pool)
    .await?;
    Ok(if stored_digest == digest {
        OutcomeKind::Unchanged
    } else {
        OutcomeKind::Conflict
    })
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

#[derive(Serialize)]
pub struct ResultRow {
    id: String,
    kind: String,
    status: String,
    verification: Verification,
}

#[derive(Serialize)]
#[serde(tag = "status", content = "reason", rename_all = "snake_case")]
enum Verification {
    Verified,
    Unverified(UnverifiedReason),
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum UnverifiedReason {
    Unregistered,
    Mismatch,
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
    let rows: Vec<(String, String, String, Option<bool>)> = sqlx::query_as(
        "select r.rule_id, r.kind, r.status, r.rule_digest = registration.digest \
         from attestation_results r \
         left join rule_registrations registration \
           on registration.source = r.rule_source and registration.path = r.rule_path \
          and registration.id = r.rule_local_id and registration.version = r.rule_version \
         where r.attestation_id = $1 order by r.position",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let results = rows
        .into_iter()
        .map(|(id, kind, status, digest_matches)| ResultRow {
            id,
            kind,
            status,
            verification: match digest_matches {
                Some(true) => Verification::Verified,
                Some(false) => Verification::Unverified(UnverifiedReason::Mismatch),
                None => Verification::Unverified(UnverifiedReason::Unregistered),
            },
        })
        .collect();
    Ok(Some(StoredAttestation { summary, results }))
}
