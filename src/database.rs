use crate::attestation::Envelope;
use crate::registration::{OutcomeKind, Rule};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;
use sqlx::migrate::Migrator;
use sqlx::postgres::PgPoolOptions;
use sqlx::types::Json;
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
pub(crate) struct RuleSummary {
    source: String,
    path: String,
    id: String,
    version: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
}

#[derive(Serialize, FromRow)]
pub(crate) struct RuleVersion {
    #[serde(flatten)]
    #[sqlx(flatten)]
    rule: RuleSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    blocking: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    knowledge: Option<Json<Box<RawValue>>>,
    digest: String,
    commit: String,
    registered_at: String,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuleScan {
    through: i64,
    pub registrations: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RuleCursor {
    pub source: String,
    pub path: String,
    pub id: String,
    pub version: String,
    pub scan: RuleScan,
}

#[derive(Serialize)]
pub(crate) struct ListedRule {
    #[serde(flatten)]
    summary: RuleSummary,
    scan: RuleScan,
}

pub(crate) async fn registered_rules(
    pool: &PgPool,
    before: Option<&RuleCursor>,
    limit: i64,
) -> Result<Option<Vec<ListedRule>>, sqlx::Error> {
    let mut transaction = pool.begin().await?;
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *transaction)
        .await?;

    // A scan uses an append-only prefix of the registrations. Its count
    // detects a late commit whose registration time falls inside the prefix.
    let (through, registrations, anchor): (Option<i64>, i64, Option<String>) = sqlx::query_as(
        "select (extract(epoch from max(registered_at)) * 1000000)::bigint, count(*), \
                (select case when version = $5 then coalesce(name, id) end \
                 from rule_registrations where source = $2 and path = $3 and id = $4 \
                   and extract(epoch from registered_at) * 1000000 <= $1::bigint \
                 order by split_part(version, '.', 1)::numeric desc, \
                          split_part(version, '.', 2)::numeric desc limit 1) \
         from rule_registrations \
         where $1::bigint is null or extract(epoch from registered_at) * 1000000 <= $1",
    )
    .bind(before.map(|cursor| cursor.scan.through))
    .bind(before.map(|cursor| &cursor.source))
    .bind(before.map(|cursor| &cursor.path))
    .bind(before.map(|cursor| &cursor.id))
    .bind(before.map(|cursor| &cursor.version))
    .fetch_one(&mut *transaction)
    .await?;
    if before.is_some_and(|cursor| {
        through != Some(cursor.scan.through)
            || registrations != cursor.scan.registrations
            || anchor.is_none()
    }) {
        return Ok(None);
    }
    let Some(through) = through else {
        return Ok(Some(Vec::new()));
    };
    let scan = RuleScan {
        through,
        registrations,
    };

    let summaries = sqlx::query_as(
        r#"with greatest as (
               select distinct on (source, path, id)
                      source, path, id, version, kind, name, description,
                      lower(coalesce(name, id) collate pg_catalog."pg_c_utf8") as sort_name
               from rule_registrations
               where extract(epoch from registered_at) * 1000000 <= $1::bigint
               order by source, path, id,
                        split_part(version, '.', 1)::numeric desc,
                        split_part(version, '.', 2)::numeric desc
           )
           select source, path, id, version, kind, name, description
           from greatest
           where $2::text is null
              or (sort_name, source collate pg_catalog."C", path collate pg_catalog."C",
                  id collate pg_catalog."C")
               > (lower($2 collate pg_catalog."pg_c_utf8"), $3, $4, $5)
           order by sort_name, source collate pg_catalog."C", path collate pg_catalog."C",
                    id collate pg_catalog."C"
           limit $6"#,
    )
    .bind(through)
    .bind(anchor)
    .bind(before.map(|cursor| &cursor.source))
    .bind(before.map(|cursor| &cursor.path))
    .bind(before.map(|cursor| &cursor.id))
    .bind(limit)
    .fetch_all(&mut *transaction)
    .await?;
    transaction.commit().await?;
    Ok(Some(
        summaries
            .into_iter()
            .map(|summary| ListedRule { summary, scan })
            .collect(),
    ))
}

pub(crate) async fn registered_rule_versions(
    pool: &PgPool,
    source: &str,
    path: &str,
    id: &str,
) -> Result<Vec<RuleVersion>, sqlx::Error> {
    sqlx::query_as(
        "select source, path, id, version, kind, name, description, blocking, prompt, knowledge, \
                digest, commit_sha as commit, \
                to_char(registered_at at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') as registered_at \
         from rule_registrations where source = $1 and path = $2 and id = $3 \
         order by split_part(version, '.', 1)::numeric, split_part(version, '.', 2)::numeric",
    )
    .bind(source)
    .bind(path)
    .bind(id)
    .fetch_all(pool)
    .await
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
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rule: Option<ResultRule>,
    verification: Verification,
}

#[derive(Serialize)]
struct ResultRule {
    source: String,
    path: String,
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
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
    let rows = sqlx::query_as(
        "select r.rule_id, r.kind, r.status, registration.name, r.rule_digest = registration.digest, \
                r.rule_source, r.rule_path, r.rule_local_id, r.rule_version \
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
        .map(
            |(id, kind, status, name, digest_matches, source, path, local_id, version)| ResultRow {
                id,
                kind,
                status,
                name,
                rule: match (source, path, local_id) {
                    (Some(source), Some(path), Some(id)) => Some(ResultRule {
                        source,
                        path,
                        id,
                        version,
                    }),
                    _ => None,
                },
                verification: match digest_matches {
                    Some(true) => Verification::Verified,
                    Some(false) => Verification::Unverified(UnverifiedReason::Mismatch),
                    None => Verification::Unverified(UnverifiedReason::Unregistered),
                },
            },
        )
        .collect();
    Ok(Some(StoredAttestation { summary, results }))
}
