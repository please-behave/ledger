use crate::attestation::Envelope;
use crate::deployment_fact::{DeploymentFact, DeploymentOutcome};
use crate::registration::{Outcome, OutcomeKind, Rule};
use crate::release_fact::ReleaseFact;
use crate::rule_digest::sha256_digest;
use crate::rule_set_registration::{MemberFailure, RuleSet, SetOutcome};
use crate::target_registration::{TargetOutcome, TargetOutcomeKind, TargetRequest};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use serde_json::value::RawValue;
use sqlx::migrate::{Migrate, Migration, Migrator};
use sqlx::postgres::PgPoolOptions;
use sqlx::types::Json;
use sqlx::{AssertSqlSafe, FromRow, PgPool, Row};
use std::collections::HashSet;
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

pub async fn migrate_with_report(pool: &PgPool) -> Result<Vec<&'static Migration>> {
    let mut connection = pool
        .acquire()
        .await
        .context("cannot acquire a migration connection")?;
    // Hold SQLx's session lock across both reads so concurrent runs cannot claim the same versions.
    connection.close_on_drop();
    connection
        .lock()
        .await
        .context("cannot lock the database for migration")?;

    let applied_versions =
        || sqlx::query_scalar::<_, i64>("select version from _sqlx_migrations where success");
    let before = match applied_versions().fetch_all(&mut *connection).await {
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("42P01") => Vec::new(),
        result => result.context("cannot read applied versions before migration")?,
    };

    MIGRATOR
        .run(&mut *connection)
        .await
        .context("cannot apply the migrations")?;

    let after = applied_versions()
        .fetch_all(&mut *connection)
        .await
        .context("cannot read applied versions after migration")?;
    Ok(MIGRATOR
        .iter()
        .filter(|migration| {
            migration.migration_type.is_up_migration()
                && after.contains(&migration.version)
                && !before.contains(&migration.version)
        })
        .collect())
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
         (id, sender, repository, commit_sha, execution_points, overall, schema_version, document, rule_sets) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(id)
    .bind(&envelope.sender)
    .bind(&attestation.repository)
    .bind(&attestation.commit)
    .bind(&attestation.execution_points)
    .bind(attestation.overall.as_str())
    .bind(attestation.schema_version)
    .bind(document)
    .bind(&attestation.rule_sets)
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

pub(crate) async fn record_release(
    pool: &PgPool,
    fact: &ReleaseFact,
) -> Result<OutcomeKind, sqlx::Error> {
    let inserted = sqlx::query(
        "insert into release_facts (repository, version, commit_sha, sender, auth_method) \
         values ($1, $2, $3, $4, $5) on conflict (repository, version) do nothing",
    )
    .bind(&fact.repository)
    .bind(&fact.version)
    .bind(&fact.commit)
    .bind(&fact.sender)
    .bind(fact.auth_method.as_str())
    .execute(pool)
    .await?;
    if inserted.rows_affected() == 1 {
        return Ok(OutcomeKind::Created);
    }

    // A separate statement sees the winner even if the insert waited for it.
    let stored_commit: String = sqlx::query_scalar(
        "select commit_sha from release_facts where repository = $1 and version = $2",
    )
    .bind(&fact.repository)
    .bind(&fact.version)
    .fetch_one(pool)
    .await?;
    Ok(if stored_commit.eq_ignore_ascii_case(&fact.commit) {
        OutcomeKind::Unchanged
    } else {
        OutcomeKind::Conflict
    })
}

pub(crate) async fn record_deployment(
    pool: &PgPool,
    fact: &DeploymentFact,
) -> Result<Option<DeploymentOutcome>, sqlx::Error> {
    let mut transaction = pool
        .begin_with("BEGIN ISOLATION LEVEL READ COMMITTED")
        .await?;
    // Lock the target, then read a fresh snapshot to see any report that held the lock first.
    let target: Option<bool> = sqlx::query_scalar(
        "select true from deployment_targets where owner = $1 and name = $2 for no key update",
    )
    .bind(&fact.sender)
    .bind(&fact.target)
    .fetch_optional(&mut *transaction)
    .await?;
    if target.is_none() {
        return Ok(None);
    }

    let current: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "select version, commit_sha from deployment_events
         where sender = $1 and target_name = $2 and repository = $3
         order by received_at desc, id desc limit 1",
    )
    .bind(&fact.sender)
    .bind(&fact.target)
    .bind(&fact.repository)
    .fetch_optional(&mut *transaction)
    .await?;
    let unchanged = current.is_some_and(|(version, commit)| match (version, commit) {
        (Some(version), _) => fact.version.as_deref() == Some(version.as_str()),
        (_, Some(commit)) => fact
            .commit
            .as_ref()
            .is_some_and(|reported| reported.eq_ignore_ascii_case(&commit)),
        _ => false,
    });
    let outcome = if unchanged {
        DeploymentOutcome::Unchanged
    } else {
        sqlx::query(
            "insert into deployment_events
                 (sender, target_name, repository, version, commit_sha, auth_method)
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(&fact.sender)
        .bind(&fact.target)
        .bind(&fact.repository)
        .bind(&fact.version)
        .bind(&fact.commit)
        .bind(fact.auth_method.as_str())
        .execute(&mut *transaction)
        .await?;
        DeploymentOutcome::Created
    };
    transaction.commit().await?;
    Ok(Some(outcome))
}

pub(crate) async fn register_targets(
    pool: &PgPool,
    request: TargetRequest,
) -> Result<Vec<TargetOutcome>, sqlx::Error> {
    let (names, descriptions): (Vec<_>, Vec<_>) = request
        .targets
        .iter()
        .map(|target| (target.name.as_str(), target.description.as_deref()))
        .unzip();

    // Keep the first entry for a name and acquire conflicting keys in name order.
    let inserted: Vec<String> = sqlx::query_scalar(
        "insert into deployment_targets (owner, name, description)
         select distinct on (name) $1, name, description
         from unnest($2::text[], $3::text[]) with ordinality as submitted(name, description, position)
         order by name, position
         on conflict (owner, name) do nothing
         returning name",
    )
    .bind(&request.sender)
    .bind(names)
    .bind(descriptions)
    .fetch_all(pool)
    .await?;

    let mut created: HashSet<_> = inserted.into_iter().collect();
    Ok(request
        .targets
        .into_iter()
        .map(|target| TargetOutcome {
            outcome: if created.remove(&target.name) {
                TargetOutcomeKind::Created
            } else {
                TargetOutcomeKind::Unchanged
            },
            name: target.name,
        })
        .collect())
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

pub(crate) async fn register_set(
    pool: &PgPool,
    sender: &str,
    commit: &str,
    set: &RuleSet,
) -> Result<SetOutcome, sqlx::Error> {
    let members =
        serde_json::to_string(&set.members).map_err(|error| sqlx::Error::Encode(error.into()))?;
    let failures = sqlx::query_as::<_, MemberFailure>(
        "select position - 1 as member_index, \
                case when rule.digest is null then 'unregistered' \
                     else 'digest_mismatch' end as reason \
         from json_array_elements($1::text::json) with ordinality as member(item, position) \
         left join rule_registrations rule \
           on rule.source = item->>'source' and rule.path = item->>'path' \
          and rule.id = item->>'id' and rule.version = item->>'version' \
         where rule.digest is distinct from item->>'digest' \
         order by position",
    )
    .bind(&members)
    .fetch_all(pool)
    .await?;
    if !failures.is_empty() {
        return Ok(SetOutcome::InvalidMembers { failures });
    }

    // Verified rule versions are append-only, so they stay valid through insertion.
    let digest = sha256_digest(members.as_bytes());
    let inserted = sqlx::query(
        "insert into rule_set_registrations \
         (source, path, id, version, name, description, knowledge, members, digest, sender, commit_sha) \
         values ($1, $2, $3, $4, $5, $6, $7::text::json, $8::text::json, $9, $10, $11) \
         on conflict (source, path, id, version) do nothing",
    )
    .bind(&set.source)
    .bind(&set.path)
    .bind(&set.id)
    .bind(&set.version)
    .bind(&set.name)
    .bind(&set.description)
    .bind(set.knowledge.as_deref().map(RawValue::get))
    .bind(&members)
    .bind(&digest)
    .bind(sender)
    .bind(commit)
    .execute(pool)
    .await?;
    let outcome = if inserted.rows_affected() == 1 {
        OutcomeKind::Created
    } else {
        let stored_digest: String = sqlx::query_scalar(
            "select digest from rule_set_registrations \
             where source = $1 and path = $2 and id = $3 and version = $4",
        )
        .bind(&set.source)
        .bind(&set.path)
        .bind(&set.id)
        .bind(&set.version)
        .fetch_one(pool)
        .await?;
        if stored_digest == digest {
            OutcomeKind::Unchanged
        } else {
            OutcomeKind::Conflict
        }
    };
    Ok(SetOutcome::Registered(Outcome { outcome, digest }))
}

#[derive(Serialize, FromRow)]
pub(crate) struct RuleSummary {
    source: String,
    path: String,
    id: String,
    version: String,
    kind: String,
    owner: String,
    rule_sets: Json<Value>,
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
pub(crate) struct RegistryScan {
    through: i64,
    pub registrations: i64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RegistryCursor {
    pub source: String,
    pub path: String,
    pub id: String,
    pub version: String,
    pub scan: RegistryScan,
}

#[derive(Serialize)]
pub(crate) struct RegistryEntry<T> {
    #[serde(flatten)]
    summary: T,
    scan: RegistryScan,
}

#[derive(Serialize, FromRow)]
pub(crate) struct RuleSetSummary {
    source: String,
    path: String,
    id: String,
    version: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    member_count: i32,
}

#[derive(Serialize, FromRow)]
pub(crate) struct RuleSetVersion {
    source: String,
    path: String,
    id: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    knowledge: Option<Json<Box<RawValue>>>,
    members: Json<Box<RawValue>>,
    digest: String,
    commit: String,
    registered_at: String,
}

enum Registry {
    Rules,
    RuleSets,
}

// Both reads match the full rule version key against the registered member list.
// The caller selects which Rule Set versions and rule rows participate.
const READ_RULE_MEMBERSHIPS: &str = r#",
    memberships as (
        select rule.source, rule.path, rule.id, rule.version,
               json_agg(json_build_object(
                   'source', sets.source, 'path', sets.path, 'id', sets.id,
                   'version', sets.version, 'name', coalesce(sets.name, sets.id)
               ) order by sets.source collate pg_catalog."C", sets.path collate pg_catalog."C",
                          sets.id collate pg_catalog."C", split_part(sets.version, '.', 1)::numeric,
                          split_part(sets.version, '.', 2)::numeric) as rule_sets
        from set_versions sets
        cross join lateral json_to_recordset(sets.members)
            as member(source text, path text, id text, version text)
        join selected_rules rule on (member.source, member.path, member.id, member.version)
                                  = (rule.source, rule.path, rule.id, rule.version)
        group by rule.source, rule.path, rule.id, rule.version
    )
    select selected_rules.*, coalesce(memberships.rule_sets, '[]'::json) as rule_sets
    from selected_rules left join memberships using (source, path, id, version)
"#;

const GREATEST_SET_VERSIONS: &str = r#",
    set_versions as (
        select distinct on (source, path, id) source, path, id, version, name, members
        from rule_set_registrations
        order by source, path, id, split_part(version, '.', 1)::numeric desc,
                 split_part(version, '.', 2)::numeric desc
    )
"#;

pub(crate) async fn registered_rules(
    pool: &PgPool,
    before: Option<&RegistryCursor>,
    limit: i64,
) -> Result<Option<Vec<RegistryEntry<RuleSummary>>>, sqlx::Error> {
    registry_page(pool, before, limit, Registry::Rules).await
}

pub(crate) async fn registered_sets(
    pool: &PgPool,
    before: Option<&RegistryCursor>,
    limit: i64,
) -> Result<Option<Vec<RegistryEntry<RuleSetSummary>>>, sqlx::Error> {
    registry_page(pool, before, limit, Registry::RuleSets).await
}

async fn registry_page<T>(
    pool: &PgPool,
    before: Option<&RegistryCursor>,
    limit: i64,
    registry: Registry,
) -> Result<Option<Vec<RegistryEntry<T>>>, sqlx::Error>
where
    T: for<'row> FromRow<'row, sqlx::postgres::PgRow> + Send + Unpin,
{
    // Only these fixed SQL fragments vary. Every request value is bound.
    let (table, fields) = match registry {
        Registry::Rules => ("rule_registrations", "kind, name, sender as owner"),
        Registry::RuleSets => (
            "rule_set_registrations",
            "coalesce(name, id) as name, json_array_length(members) as member_count",
        ),
    };
    let mut transaction = pool.begin().await?;
    sqlx::query("set transaction isolation level repeatable read read only")
        .execute(&mut *transaction)
        .await?;

    // A scan uses an append-only prefix of the registrations. Its count
    // detects a late commit whose registration time falls inside the prefix.
    let scan_query = format!(
        "select (extract(epoch from max(registered_at)) * 1000000)::bigint, count(*), \
                (select case when version = $5 then coalesce(name, id) end \
                 from {table} where source = $2 and path = $3 and id = $4 \
                   and extract(epoch from registered_at) * 1000000 <= $1::bigint \
                 order by split_part(version, '.', 1)::numeric desc, \
                          split_part(version, '.', 2)::numeric desc limit 1) \
         from {table} \
         where $1::bigint is null or extract(epoch from registered_at) * 1000000 <= $1",
    );
    let (through, registrations, anchor): (Option<i64>, i64, Option<String>) =
        sqlx::query_as(AssertSqlSafe(scan_query))
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
    let scan = RegistryScan {
        through,
        registrations,
    };

    let (set_versions, select_page) = match registry {
        Registry::Rules => (GREATEST_SET_VERSIONS, READ_RULE_MEMBERSHIPS),
        Registry::RuleSets => ("", "select selected_rules.* from selected_rules"),
    };
    let page_query = format!(
        r#"with greatest as (
               select distinct on (source, path, id)
                      source, path, id, version, description, {fields},
                      lower(coalesce(name, id) collate pg_catalog."pg_c_utf8") as sort_name
               from {table}
               where extract(epoch from registered_at) * 1000000 <= $1::bigint
               order by source, path, id,
                        split_part(version, '.', 1)::numeric desc,
                        split_part(version, '.', 2)::numeric desc
           ), selected_rules as (
               select *
               from greatest
               where $2::text is null
                  or (sort_name, source collate pg_catalog."C", path collate pg_catalog."C",
                      id collate pg_catalog."C")
                   > (lower($2 collate pg_catalog."pg_c_utf8"), $3, $4, $5)
               order by sort_name, source collate pg_catalog."C", path collate pg_catalog."C",
                        id collate pg_catalog."C"
               limit $6
           ){set_versions}
           {select_page}
           order by sort_name, selected_rules.source collate pg_catalog."C",
                    selected_rules.path collate pg_catalog."C", selected_rules.id collate pg_catalog."C""#,
    );
    let summaries = sqlx::query_as(AssertSqlSafe(page_query))
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
            .map(|summary| RegistryEntry { summary, scan })
            .collect(),
    ))
}

pub(crate) async fn registered_rule_versions(
    pool: &PgPool,
    source: &str,
    path: &str,
    id: &str,
) -> Result<Vec<RuleVersion>, sqlx::Error> {
    let query = format!(
        r#"with selected_rules as (
               select source, path, id, version, kind, name, description, blocking, prompt, knowledge,
                      sender as owner, digest, commit_sha as commit,
                      to_char(registered_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') as registered_at
               from rule_registrations where source = $1 and path = $2 and id = $3
           ), set_versions as (
               select source, path, id, version, name, members from rule_set_registrations
           )
           {READ_RULE_MEMBERSHIPS}
           order by split_part(selected_rules.version, '.', 1)::numeric,
                    split_part(selected_rules.version, '.', 2)::numeric"#,
    );
    sqlx::query_as(AssertSqlSafe(query))
        .bind(source)
        .bind(path)
        .bind(id)
        .fetch_all(pool)
        .await
}

pub(crate) async fn registered_set_versions(
    pool: &PgPool,
    source: &str,
    path: &str,
    id: &str,
) -> Result<Vec<RuleSetVersion>, sqlx::Error> {
    sqlx::query_as(
        r#"select source, path, id, version, name, description, knowledge, members,
                  digest, commit_sha as commit,
                  to_char(registered_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') as registered_at
           from rule_set_registrations where source = $1 and path = $2 and id = $3
           order by split_part(version, '.', 1)::numeric, split_part(version, '.', 2)::numeric"#,
    )
    .bind(source)
    .bind(path)
    .bind(id)
    .fetch_all(pool)
    .await
}

#[derive(Serialize, FromRow)]
pub(crate) struct RuleSetCompliance {
    repository: String,
    version: String,
    registered: bool,
    complete: bool,
    non_passing_members: Json<Value>,
}

#[derive(Serialize, FromRow)]
pub(crate) struct CompletedRuleSets {
    repository: String,
    sets: Json<Value>,
}

macro_rules! rule_set_compliance_query {
    ($binding_filter:literal, $select:literal) => {
        rule_set_compliance_query!(
            "select distinct on (repository) repository, id, rule_sets
             from attestations
             where execution_points @> array['check']
             order by repository, received_at desc, id desc",
            $binding_filter,
            $select
        )
    };
    ($latest_check:literal, $binding_filter:literal, $select:literal) => {
        rule_set_compliance_query!("", $latest_check, $binding_filter, $select)
    };
    ($prefix:expr, $latest_check:literal, $binding_filter:literal, $select:literal) => {
        concat!(
            "with ",
            $prefix,
            "latest_check as (",
            $latest_check,
            "), bindings as (
                 select distinct a.repository, a.id as attestation_id,
                        binding->>'source' as source, binding->>'path' as path,
                        binding->>'id' as id, binding->>'version' as version
                 from latest_check a
                 cross join lateral jsonb_array_elements(a.rule_sets) as binding
                 where binding->'execution_points' ? 'check' ",
            $binding_filter,
            "
             ), compliance as (
                 select b.*, coalesce(s.name, b.id) as name, s.version is not null as registered,
                        s.version is not null and failures.members is null as complete,
                        coalesce(failures.members, '[]'::json) as non_passing_members
                 from bindings b
                 left join rule_set_registrations s using (source, path, id, version)
                 cross join lateral (
                     select json_agg(json_build_object(
                         'source', member->>'source', 'path', member->>'path',
                         'id', member->>'id', 'version', member->>'version',
                         'status', coalesce(result.status, 'NOT_RUN')
                     ) order by position, result.status) as members
                     from json_array_elements(s.members) with ordinality as m(member, position)
                     left join lateral (
                         select distinct r.status
                         from attestation_results r
                         where r.attestation_id = b.attestation_id
                           and r.rule_source = member->>'source'
                           and r.rule_path = member->>'path'
                           and r.rule_local_id = member->>'id'
                     ) result on true
                     where result.status is distinct from 'PASS'
                 ) failures
             ) ",
            $select
        )
    };
}

pub(crate) async fn rule_set_compliance(
    pool: &PgPool,
    source: &str,
    path: &str,
    id: &str,
) -> Result<Vec<RuleSetCompliance>, sqlx::Error> {
    sqlx::query_as(rule_set_compliance_query!(
        "and (binding->>'source', binding->>'path', binding->>'id') = ($1, $2, $3)",
        "select repository, version, registered, complete, non_passing_members
         from compliance order by repository, version"
    ))
    .bind(source)
    .bind(path)
    .bind(id)
    .fetch_all(pool)
    .await
}

pub(crate) async fn completed_rule_sets(
    pool: &PgPool,
) -> Result<Vec<CompletedRuleSets>, sqlx::Error> {
    sqlx::query_as(rule_set_compliance_query!(
        "",
        "select repository, json_agg(json_build_object(
             'source', source, 'path', path, 'id', id, 'version', version, 'name', name
         ) order by source, path, id, version) as sets
         from compliance where complete
         group by repository order by repository"
    ))
    .fetch_all(pool)
    .await
}

// Select one attestation for each requested pair before evaluating its bindings.
macro_rules! commit_evidence_query {
    ($prefix:expr, $select:literal) => {
        rule_set_compliance_query!(
            $prefix,
            "select k.repository, k.commit_sha, a.*
             from evidence_keys k
             cross join lateral (
                 select id, overall, received_at, sender, rule_sets,
                        document->>'auth_method' as auth_method
                 from attestations
                 where repository = k.repository and commit_sha = k.commit_sha
                   and execution_points @> array['check']
                 order by received_at desc, id desc limit 1
             ) a",
            "",
            $select
        )
    };
}

#[derive(Serialize, FromRow)]
pub(crate) struct CommitEvidence {
    id: String,
    overall: String,
    received_at: String,
    sender: String,
    auth_method: Option<String>,
    rule_sets: Json<Value>,
}

pub(crate) async fn commit_evidence(
    pool: &PgPool,
    repository: &str,
    commit: &str,
) -> Result<Option<CommitEvidence>, sqlx::Error> {
    sqlx::query_as(commit_evidence_query!(
        "evidence_keys as (select $1::text as repository, $2::text as commit_sha), ",
        r#"select id::text as id, overall, sender, auth_method,
                  to_char(received_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') as received_at,
                  coalesce((
                      select json_agg(json_build_object(
                          'source', source, 'path', path, 'id', id, 'version', version, 'name', name,
                          'registered', registered, 'complete', complete,
                          'non_passing_members', non_passing_members
                      ) order by source, path, id, version)
                      from compliance
                  ), '[]'::json) as rule_sets
           from latest_check"#
    ))
    .bind(repository)
    .bind(commit)
    .fetch_optional(pool)
    .await
}

#[derive(Serialize, FromRow)]
pub(crate) struct Release {
    repository: String,
    version: String,
    commit: String,
    received_at: String,
    sender: String,
    auth_method: String,
}

#[derive(Serialize, FromRow)]
pub(crate) struct ReleaseSummary {
    #[serde(flatten)]
    #[sqlx(flatten)]
    release: Release,
    evidence: Option<Json<Value>>,
}

#[derive(Serialize)]
pub(crate) struct ReleaseDetail {
    #[serde(flatten)]
    release: Release,
    evidence: Option<CommitEvidence>,
}

pub(crate) async fn release_detail(
    pool: &PgPool,
    repository: &str,
    version: &str,
) -> Result<Option<ReleaseDetail>, sqlx::Error> {
    let release: Option<Release> = sqlx::query_as(
        r#"select repository, version, commit_sha as commit, sender, auth_method,
                  to_char(received_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') as received_at
           from release_facts where repository = $1 and version = $2"#,
    )
    .bind(repository)
    .bind(version)
    .fetch_optional(pool)
    .await?;
    let Some(release) = release else {
        return Ok(None);
    };
    let evidence = commit_evidence(pool, repository, &release.commit).await?;
    Ok(Some(ReleaseDetail { release, evidence }))
}

macro_rules! release_page_query {
    ($candidates:literal) => {
        commit_evidence_query!(
            concat!(
                "candidates as (", $candidates, "),
                 page as (
                     select * from candidates r
                     where ($2::text is null or (r.received_at, r.repository, r.version) < (
                         select received_at, repository, version
                         from release_facts where repository = $2 and version = $3
                     ))
                     order by r.received_at desc, r.repository desc, r.version desc
                     limit $4
                 ), evidence_keys as (select distinct repository, commit_sha from page), "
            ),
            r#"select r.repository, r.version, r.commit_sha as commit, r.sender, r.auth_method,
                      to_char(r.received_at at time zone 'utc', 'YYYY-MM-DD"T"HH24:MI:SS.US"Z"') as received_at,
                      case when a.id is not null then json_build_object(
                          'overall', a.overall,
                          'completed_rule_sets', coalesce((
                              select json_agg(json_build_object(
                                  'source', source, 'path', path, 'id', id, 'version', version, 'name', name
                              ) order by source, path, id, version)
                              from compliance where attestation_id = a.id and complete
                          ), '[]'::json)
                      ) end as evidence
               from page r left join latest_check a using (repository, commit_sha)
               order by r.received_at desc, r.repository desc, r.version desc"#
        )
    };
}

pub(crate) async fn release_page(
    pool: &PgPool,
    repository: Option<&str>,
    before: Option<(&str, &str)>,
    limit: i64,
) -> Result<Option<Vec<ReleaseSummary>>, sqlx::Error> {
    if let Some((repository, version)) = before {
        let exists: bool = sqlx::query_scalar(
            "select exists(select from release_facts where repository = $1 and version = $2)",
        )
        .bind(repository)
        .bind(version)
        .fetch_one(pool)
        .await?;
        if !exists {
            return Ok(None);
        }
    }
    let query = if repository.is_some() {
        release_page_query!("select * from release_facts where repository = $1")
    } else {
        release_page_query!(
            "select distinct on (repository) * from release_facts where $1::text is null
             order by repository, received_at desc, version desc"
        )
    };
    sqlx::query_as(query)
        .bind(repository)
        .bind(before.map(|(repository, _)| repository))
        .bind(before.map(|(_, version)| version))
        .bind(limit)
        .fetch_all(pool)
        .await
        .map(Some)
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
    rule_sets: Value,
    results: Vec<ResultRow>,
}

macro_rules! summary_query {
    ($source:literal $(, $extra_columns:literal)?) => {
        concat!(
            "select a.id::text as id, \
             to_char(a.received_at at time zone 'utc', 'YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') as received_at, \
             a.sender, a.repository, a.commit_sha as \"commit\", a.execution_points, a.overall, \
             (select count(*) from attestation_results r \
              where r.attestation_id = a.id and r.status = 'WARN') as warn_count, \
             (select count(*) from attestation_results r \
              where r.attestation_id = a.id and r.status = 'NOT_RUN') as not_run_count, \
             (select count(*) from attestation_results r \
              where r.attestation_id = a.id) as result_count",
            $($extra_columns,)?
            " from ",
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
    repository: Option<&str>,
) -> Result<Vec<Summary>, sqlx::Error> {
    sqlx::query_as(summary_query!(
        "(select * from attestations where ($1::uuid is null or id < $1) \
          and ($3::text is null or repository = $3) \
          order by id desc limit $2) a \
         order by a.id desc"
    ))
    .bind(before)
    .bind(limit)
    .bind(repository)
    .fetch_all(pool)
    .await
}

pub async fn attestation(
    pool: &PgPool,
    id: Uuid,
) -> Result<Option<StoredAttestation>, sqlx::Error> {
    let attestation = sqlx::query(summary_query!(
        "attestations a where a.id = $1",
        ", a.rule_sets"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some(attestation) = attestation else {
        return Ok(None);
    };
    let summary = Summary::from_row(&attestation)?;
    let rule_sets = attestation.try_get("rule_sets")?;
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
    Ok(Some(StoredAttestation {
        summary,
        rule_sets,
        results,
    }))
}
