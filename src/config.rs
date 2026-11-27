use anyhow::{Context, Result, ensure};
use chrono::TimeDelta;
use std::net::SocketAddr;

const DATABASE_URL: &str = "LEDGER_DATABASE_URL";
const LISTEN_ADDRESS: &str = "LEDGER_LISTEN_ADDRESS";
const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:8080";
const HEARTBEAT_STALE_SECONDS: &str = "LEDGER_OVERVIEW_HEARTBEAT_STALE_SECONDS";
const GRACE_SECONDS: &str = "LEDGER_OVERVIEW_GRACE_SECONDS";
const OVERDUE_STEP_SECONDS: &str = "LEDGER_OVERVIEW_OVERDUE_STEP_SECONDS";

pub fn database_url() -> Result<String> {
    std::env::var(DATABASE_URL).with_context(|| format!("{DATABASE_URL} must be set"))
}

pub fn listen_address() -> Result<SocketAddr> {
    let value = std::env::var(LISTEN_ADDRESS).unwrap_or_else(|_| DEFAULT_LISTEN_ADDRESS.into());
    value.parse().with_context(|| {
        format!("{LISTEN_ADDRESS} must be an address such as {DEFAULT_LISTEN_ADDRESS}")
    })
}

#[derive(Clone)]
pub struct OverviewConfig {
    pub(crate) heartbeat_staleness: TimeDelta,
    pub(crate) grace_seconds: i64,
    pub(crate) overdue_step_seconds: Vec<i64>,
}

impl Default for OverviewConfig {
    fn default() -> Self {
        Self {
            heartbeat_staleness: TimeDelta::hours(1),
            grace_seconds: 3600,
            overdue_step_seconds: vec![0, 86_400, 259_200, 604_800],
        }
    }
}

impl OverviewConfig {
    pub fn with_heartbeat_staleness(mut self, duration: TimeDelta) -> Self {
        self.heartbeat_staleness = duration;
        self
    }

    pub fn from_env() -> Result<Self> {
        let mut config = Self::default();
        if let Some(value) = optional_env(HEARTBEAT_STALE_SECONDS)? {
            let threshold = seconds(&value, HEARTBEAT_STALE_SECONDS)?;
            config.heartbeat_staleness = TimeDelta::try_seconds(threshold)
                .filter(|duration| *duration > TimeDelta::zero())
                .with_context(|| {
                    format!("{HEARTBEAT_STALE_SECONDS} must be a positive duration")
                })?;
        }
        if let Some(value) = optional_env(GRACE_SECONDS)? {
            config.grace_seconds = seconds(&value, GRACE_SECONDS)?;
        }
        if let Some(value) = optional_env(OVERDUE_STEP_SECONDS)? {
            config.overdue_step_seconds = value
                .split(',')
                .map(|step| seconds(step, OVERDUE_STEP_SECONDS))
                .collect::<Result<_>>()?;
            ensure!(
                config.overdue_step_seconds.first() == Some(&0)
                    && config
                        .overdue_step_seconds
                        .is_sorted_by(|left, right| left < right),
                "{OVERDUE_STEP_SECONDS} must start at zero and strictly increase"
            );
        }
        Ok(config)
    }
}

fn optional_env(name: &str) -> Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {name}")),
    }
}

fn seconds(value: &str, name: &str) -> Result<i64> {
    let seconds = value
        .trim()
        .parse::<i64>()
        .with_context(|| format!("{name} must contain whole seconds"))?;
    ensure!(seconds >= 0, "{name} must contain nonnegative seconds");
    Ok(seconds)
}
