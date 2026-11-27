mod dataset;
mod reads;

use anyhow::{Context, Result};
use chrono::{TimeDelta, Utc};
use clap::Parser;
use dataset::Dataset;
use ledger::{config::OverviewConfig, database, http};
use sqlx::{
    AssertSqlSafe, Connection, PgConnection,
    postgres::{PgConnectOptions, PgPoolOptions},
};
use std::{
    num::NonZeroU32,
    time::{Duration, Instant},
};
use tracing_subscriber::{filter::filter_fn, layer::SubscriberExt};
use uuid::Uuid;

#[derive(Parser)]
#[command(about = "Seed and benchmark the ledger reads in a new PostgreSQL schema")]
struct Args {
    /// PostgreSQL URL for a disposable database, using a role that can load auto_explain.
    #[arg(long)]
    database_url: String,
    #[arg(long, default_value_t = 1)]
    seed: u64,
    #[arg(long, default_value_t = NonZeroU32::new(dataset::REPOSITORIES).unwrap())]
    repositories: NonZeroU32,
    #[arg(long, default_value_t = NonZeroU32::new(dataset::ATTESTATIONS).unwrap())]
    attestations: NonZeroU32,
    #[arg(long, default_value = "20")]
    repeats: NonZeroU32,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let options: PgConnectOptions = args
        .database_url
        .parse()
        .context("invalid PostgreSQL URL")?;
    let mut setup = PgConnection::connect_with(&options)
        .await
        .context("cannot connect to PostgreSQL")?;
    // Check profiling support before creating anything in the database.
    sqlx::query("load 'auto_explain'")
        .execute(&mut setup)
        .await
        .context("the connection role must be allowed to load PostgreSQL auto_explain")?;
    let schema = format!("ledger_benchmark_{}", Uuid::now_v7().simple());
    sqlx::query(AssertSqlSafe(format!("create schema {schema}")))
        .execute(&mut setup)
        .await
        .context("cannot create the benchmark schema")?;
    setup.close().await?;
    println!("Benchmark schema: {schema}");

    // Keep one session for both the timed reads and the profiled replay.
    let pool = PgPoolOptions::new()
        .max_connections(1)
        .idle_timeout(None)
        .max_lifetime(None)
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("load 'auto_explain'")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect_with(options.options([("search_path", schema.as_str()), ("timezone", "UTC")]))
        .await?;
    database::migrate(&pool).await?;

    let data = Dataset::generate(args.seed, args.repositories.get(), args.attestations.get());
    let mut transaction = pool.begin().await?;
    sqlx::query("create temporary table fixture on commit drop as select $1::jsonb as data, $2::timestamptz as epoch")
        .bind(sqlx::types::Json(&data))
        .bind(dataset::epoch())
        .execute(&mut *transaction)
        .await?;
    sqlx::raw_sql(include_str!("seed.sql"))
        .execute(&mut *transaction)
        .await
        .context("cannot seed the benchmark schema")?;
    transaction.commit().await?;
    // Analyze the fixture's remaining tables without touching other schemas.
    sqlx::raw_sql(
        "analyze attestations, attestation_results, rule_registrations, rule_set_registrations, release_facts, \
         deployment_events, deployment_targets, repository_inventory, repository_head_observations, collector_heartbeats",
    ).execute(&pool).await?;
    println!(
        "Seed {}: {} repositories, {} attestations",
        args.seed,
        data.repositories.len(),
        data.attestations.len()
    );

    // Keep the fixed fixture heartbeat fresh for this run while retaining the normal grace period.
    let heartbeat_staleness =
        (Utc::now() - dataset::epoch()).max(TimeDelta::zero()) + TimeDelta::hours(1);
    let router = http::router_with_overview_config(
        pool.clone(),
        OverviewConfig::default().with_heartbeat_staleness(heartbeat_staleness),
    );
    let sample = data
        .attestations
        .last()
        .context("the fixture has no attestations")?;
    let cases = reads::cases(&router, sample).await?;
    let mut timings = Vec::with_capacity(cases.len());
    println!(
        "Median ms     p95 ms  Read ({} timed requests after warm-up)",
        args.repeats
    );
    for uri in cases {
        reads::get(&router, &uri).await?;
        let mut samples = Vec::with_capacity(args.repeats.get() as usize);
        for _ in 0..args.repeats.get() {
            let start = Instant::now();
            reads::get(&router, &uri).await?;
            samples.push(start.elapsed());
        }
        let (median, p95) = summarize(&mut samples);
        println!(
            "{:9.3}  {:9.3}  {uri}",
            milliseconds(median),
            milliseconds(p95)
        );
        timings.push((median, uri));
    }

    // Enable PostgreSQL's EXPLAIN (ANALYZE, BUFFERS) instrumentation only for replay.
    sqlx::raw_sql(
        "set auto_explain.log_analyze = on; set auto_explain.log_buffers = on; \
         set auto_explain.log_level = notice; set client_min_messages = notice; \
         set auto_explain.log_min_duration = 0",
    )
    .execute(&pool)
    .await?;
    let notices = tracing_subscriber::fmt::layer()
        .without_time()
        .with_level(false)
        .with_target(false)
        .with_ansi(false)
        .with_writer(std::io::stdout);
    let subscriber = tracing_subscriber::registry()
        .with(notices)
        .with(filter_fn(|metadata| {
            metadata.target() == "sqlx::postgres::notice"
        }));
    tracing::subscriber::set_global_default(subscriber)?;
    timings.sort_unstable_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    for (_, uri) in timings.iter().take(3) {
        println!("\nEXPLAIN (ANALYZE, BUFFERS) via auto_explain: GET {uri}");
        reads::get(&router, uri).await?;
    }
    Ok(())
}

fn summarize(samples: &mut [Duration]) -> (Duration, Duration) {
    samples.sort_unstable();
    let count = samples.len();
    let middle = count / 2;
    let median = if count.is_multiple_of(2) {
        let lower = samples[middle - 1];
        lower + (samples[middle] - lower) / 2
    } else {
        samples[middle]
    };
    let p95 = samples[count.saturating_mul(95).div_ceil(100) - 1];
    (median, p95)
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
