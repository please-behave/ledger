use anyhow::{Result, ensure};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use sqlx::postgres::PgConnection;
use sqlx::types::Json;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

static VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema = serde_json::from_str(include_str!(
        "../why/contracts/ingest/platform-sync.schema.json"
    ))
    .expect("bundled platform sync schema is JSON");
    jsonschema::options()
        .should_validate_formats(true)
        .build(&schema)
        .expect("bundled platform sync schema is valid")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PlatformSync {
    sender: String,
    platform: String,
    host: String,
    scope: String,
    completed_at: DateTime<Utc>,
    inventory: Vec<InventoryEntry>,
    heads: Vec<HeadObservation>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct InventoryEntry {
    name: String,
    default_branch: String,
    archived: bool,
    fork: bool,
    last_activity_at: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct HeadObservation {
    name: String,
    branch: String,
    commit_sha: String,
    committed_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
}

impl PlatformSync {
    pub(crate) fn parse(body: &[u8]) -> Result<Self> {
        // Typed decoding rejects duplicate keys, which a JSON value would silently collapse.
        let mut sync: Self = serde_json::from_slice(body)?;
        let document = serde_json::from_slice(body)?;
        ensure!(VALIDATOR.is_valid(&document), "invalid platform sync");

        sync.host.make_ascii_lowercase();
        if sync.platform == "github" {
            sync.scope.make_ascii_lowercase();
            for entry in &mut sync.inventory {
                entry.name.make_ascii_lowercase();
            }
            for head in &mut sync.heads {
                head.name.make_ascii_lowercase();
            }
        }
        for head in &mut sync.heads {
            head.commit_sha.make_ascii_lowercase();
        }

        let (_, scoped_name) = sync.scope_parts();
        let mut branches = HashMap::new();
        for entry in &sync.inventory {
            ensure!(
                scoped_name.is_none_or(|name| name == entry.name),
                "inventory entry outside the scope"
            );
            ensure!(
                branches
                    .insert(entry.name.as_str(), entry.default_branch.as_str())
                    .is_none(),
                "duplicate inventory entry"
            );
        }
        let mut observed = HashSet::new();
        for head in &sync.heads {
            ensure!(
                branches.get(head.name.as_str()).copied() == Some(head.branch.as_str()),
                "head must match an inventory entry's default branch"
            );
            ensure!(observed.insert(&head.name), "duplicate head observation");
            ensure!(
                head.observed_at <= sync.completed_at,
                "head observed after the sync completed"
            );
        }
        Ok(sync)
    }

    fn scope_parts(&self) -> (&str, Option<&str>) {
        let (owner, name) = self.scope.split_once('/').expect("validated scope");
        (owner, (name != "*").then_some(name))
    }

    pub(crate) async fn store(&self, pool: &PgPool) -> Result<(), sqlx::Error> {
        let (owner, scoped_name) = self.scope_parts();
        let mut transaction = pool.begin().await?;
        // An owner-wide scope overlaps every single-repository scope for the same owner.
        sqlx::query(
            "select pg_advisory_xact_lock(hashtextextended(
                 concat_ws('/', 'ledger/platform-sync', $1::text, $2::text, $3::text), 0))",
        )
        .bind(&self.platform)
        .bind(&self.host)
        .bind(owner)
        .execute(&mut *transaction)
        .await?;

        let advanced = sqlx::query(
            "insert into collector_heartbeats (sender, platform, host, scope, completed_at)
             values ($1, $2, $3, $4, $5)
             on conflict (sender, platform, host, scope) do update
                 set completed_at = excluded.completed_at
                 where collector_heartbeats.completed_at < excluded.completed_at",
        )
        .bind(&self.sender)
        .bind(&self.platform)
        .bind(&self.host)
        .bind(&self.scope)
        .bind(self.completed_at)
        .execute(&mut *transaction)
        .await?
        .rows_affected()
            > 0;

        if advanced {
            self.store_inventory(&mut transaction, owner, scoped_name)
                .await?;
            self.store_heads(&mut transaction, owner).await?;
        }
        transaction.commit().await
    }

    async fn store_inventory(
        &self,
        connection: &mut PgConnection,
        owner: &str,
        scoped_name: Option<&str>,
    ) -> Result<(), sqlx::Error> {
        // A newer covering heartbeat already in storage omitted any repository whose
        // last sighting predates it. Preserve that omission when a late sighting arrives.
        sqlx::query(
            "insert into repository_inventory as current
                 (platform, host, owner, name, sender, default_branch, archived, fork,
                  first_seen_at, last_seen_at, last_activity_at, gone_at, gone_sender)
             select $1, $2, $3, entry.name, $4, entry.default_branch, entry.archived, entry.fork,
                    $5, $5, entry.last_activity_at, omission.completed_at, omission.sender
             from jsonb_to_recordset($6) as entry(name text, default_branch text,
                 archived boolean, fork boolean, last_activity_at timestamptz)
             left join lateral (
                 select completed_at, sender from collector_heartbeats
                 where platform = $1 and host = $2 and completed_at > $5
                   and scope in ($3 || '/*', $3 || '/' || entry.name)
                 order by completed_at, sender
                 limit 1
             ) as omission on true
             on conflict (platform, host, owner, name) do update
             set sender = excluded.sender,
                 default_branch = excluded.default_branch,
                 archived = excluded.archived,
                 fork = excluded.fork,
                 last_seen_at = excluded.last_seen_at,
                 last_activity_at = excluded.last_activity_at,
                 gone_at = case when current.gone_at > excluded.last_seen_at
                           then least(current.gone_at, excluded.gone_at) else excluded.gone_at end,
                 gone_sender = case when current.gone_at > excluded.last_seen_at
                                    and (excluded.gone_at is null or current.gone_at <= excluded.gone_at)
                               then current.gone_sender else excluded.gone_sender end
             where current.last_seen_at < excluded.last_seen_at",
        )
        .bind(&self.platform)
        .bind(&self.host)
        .bind(owner)
        .bind(&self.sender)
        .bind(self.completed_at)
        .bind(Json(&self.inventory))
        .execute(&mut *connection)
        .await?;

        let names: Vec<&str> = self
            .inventory
            .iter()
            .map(|entry| entry.name.as_str())
            .collect();
        sqlx::query(
            "update repository_inventory set first_seen_at = $4
             where platform = $1 and host = $2 and owner = $3 and first_seen_at > $4
               and name = any($5)",
        )
        .bind(&self.platform)
        .bind(&self.host)
        .bind(owner)
        .bind(self.completed_at)
        .bind(&names)
        .execute(&mut *connection)
        .await?;

        sqlx::query(
            "update repository_inventory set gone_at = $4, gone_sender = $5
             where platform = $1 and host = $2 and owner = $3 and last_seen_at < $4
               and (gone_at is null or gone_at > $4)
               and ($6::text is null or name = $6) and name <> all($7)",
        )
        .bind(&self.platform)
        .bind(&self.host)
        .bind(owner)
        .bind(self.completed_at)
        .bind(&self.sender)
        .bind(scoped_name)
        .bind(&names)
        .execute(connection)
        .await?;
        Ok(())
    }

    async fn store_heads(
        &self,
        connection: &mut PgConnection,
        owner: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "insert into repository_head_observations
                 (platform, host, owner, name, sender, branch, commit_sha, committed_at, observed_at)
             select $1, $2, $3, head.name, $4, head.branch, head.commit_sha, head.committed_at,
                    head.observed_at
             from jsonb_to_recordset($5) as head(name text, branch text, commit_sha text,
                 committed_at timestamptz, observed_at timestamptz)
             where head.commit_sha is distinct from (
                 select previous.commit_sha
                 from repository_head_observations previous
                 where (previous.platform, previous.host, previous.owner, previous.name,
                        previous.branch) = ($1, $2, $3, head.name, head.branch)
                   and previous.observed_at < head.observed_at
                 order by previous.observed_at desc, previous.commit_sha desc
                 limit 1
             )
             on conflict do nothing",
        )
        .bind(&self.platform)
        .bind(&self.host)
        .bind(owner)
        .bind(&self.sender)
        .bind(Json(&self.heads))
        .execute(connection)
        .await?;
        Ok(())
    }
}
