use crate::config::OverviewConfig;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool, types::Json};

#[derive(Serialize)]
pub(crate) struct RepositoryOverview {
    heartbeat: Heartbeat,
    repositories: Vec<RepositoryEntry>,
}

#[derive(Serialize)]
struct Heartbeat {
    completed_at: Option<DateTime<Utc>>,
    stale: bool,
}

#[derive(Serialize)]
struct RepositoryEntry {
    repository: String,
    platform: Option<String>,
    managed: bool,
    state: RepositoryState,
    status_token: &'static str,
    heartbeat_completed_at: Option<DateTime<Utc>>,
    latest_check: Option<CheckVerdict>,
    head: Option<ObservedHead>,
    pending_since: Option<DateTime<Utc>>,
    pending_age_seconds: Option<i64>,
    overdue_step: Option<usize>,
    overdue_steps: Option<usize>,
    observed_heads_since_evidence: i64,
    last_evidence_at: Option<DateTime<Utc>>,
    latest_release: Option<Release>,
    latest_deployment: Option<Deployment>,
}

#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum RepositoryState {
    Unknown,
    NeverIntegrated,
    Current,
    Pending,
    Overdue,
}

impl RepositoryState {
    fn token(self) -> &'static str {
        match self {
            Self::Unknown => "status-not-run",
            Self::NeverIntegrated => "status-failed",
            Self::Current => "status-passed",
            Self::Pending => "status-allowed",
            Self::Overdue => "status-warning",
        }
    }
}

#[derive(Deserialize, Serialize)]
struct CheckVerdict {
    id: String,
    commit: String,
    overall: String,
    received_at: DateTime<Utc>,
}

#[derive(Deserialize, Serialize)]
struct ObservedHead {
    branch: String,
    commit: String,
    committed_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
    direct_evidence: bool,
}

#[derive(Deserialize, Serialize)]
struct Release {
    version: String,
    commit: String,
    received_at: DateTime<Utc>,
}

#[derive(Deserialize, Serialize)]
struct Deployment {
    owner: String,
    name: String,
    received_at: DateTime<Utc>,
}

#[derive(Deserialize)]
struct RepositoryFacts {
    repository: String,
    platform: Option<String>,
    completed_at: Option<DateTime<Utc>>,
    latest_check: Option<CheckVerdict>,
    head: Option<ObservedHead>,
    first_uncovered_at: Option<DateTime<Utc>>,
    observed_heads_since_evidence: i64,
    latest_release: Option<Release>,
    latest_deployment: Option<Deployment>,
}

impl RepositoryFacts {
    fn entry(self, now: DateTime<Utc>, config: &OverviewConfig) -> RepositoryEntry {
        let last_evidence_at = self.latest_check.as_ref().map(|check| check.received_at);
        let pending_since = last_evidence_at.and(self.first_uncovered_at);
        let pending_age_seconds = pending_since.map(|time| (now - time).num_seconds().max(0));
        let (state, overdue_step) = self.status(now, config, pending_age_seconds);
        RepositoryEntry {
            repository: self.repository,
            managed: self.platform.is_some(),
            platform: self.platform,
            state,
            status_token: state.token(),
            heartbeat_completed_at: self.completed_at,
            latest_check: self.latest_check,
            head: self.head,
            pending_since,
            pending_age_seconds,
            overdue_step,
            overdue_steps: overdue_step.map(|_| config.overdue_step_seconds.len()),
            observed_heads_since_evidence: self.observed_heads_since_evidence,
            last_evidence_at,
            latest_release: self.latest_release,
            latest_deployment: self.latest_deployment,
        }
    }

    fn status(
        &self,
        now: DateTime<Utc>,
        config: &OverviewConfig,
        pending_age_seconds: Option<i64>,
    ) -> (RepositoryState, Option<usize>) {
        use RepositoryState::*;
        match (&self.latest_check, &self.head, pending_age_seconds) {
            _ if stale(self.completed_at, now, config) => (Unknown, None),
            (None, _, _) => (NeverIntegrated, None),
            (_, None, _) => (Unknown, None),
            (_, Some(head), _) if head.direct_evidence => (Current, None),
            (_, _, Some(age)) if age >= config.grace_seconds => {
                let overdue_seconds = age - config.grace_seconds;
                let step = config
                    .overdue_step_seconds
                    .partition_point(|step| *step <= overdue_seconds);
                (Overdue, Some(step))
            }
            (_, _, Some(_)) => (Pending, None),
            _ => (Unknown, None),
        }
    }
}

fn stale(completed_at: Option<DateTime<Utc>>, now: DateTime<Utc>, config: &OverviewConfig) -> bool {
    completed_at.is_none_or(|time| now - time > config.heartbeat_staleness)
}

#[derive(FromRow)]
struct OverviewRow {
    now: DateTime<Utc>,
    oldest_sync: Option<DateTime<Utc>>,
    facts: Option<Json<RepositoryFacts>>,
}

pub(crate) async fn read(
    pool: &PgPool,
    config: &OverviewConfig,
) -> Result<RepositoryOverview, sqlx::Error> {
    let rows: Vec<OverviewRow> = sqlx::query_as(include_str!("repository_overview.sql"))
        .fetch_all(pool)
        .await?;
    // The statement returns its metadata even when there are no repositories.
    let snapshot = rows.first().ok_or(sqlx::Error::RowNotFound)?;
    Ok(RepositoryOverview {
        heartbeat: Heartbeat {
            completed_at: snapshot.oldest_sync,
            stale: stale(snapshot.oldest_sync, snapshot.now, config),
        },
        repositories: rows
            .into_iter()
            .filter_map(|row| row.facts.map(|Json(facts)| facts.entry(row.now, config)))
            .collect(),
    })
}
