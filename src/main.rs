use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ledger::{config, database, http};
use tokio::net::TcpListener;

#[derive(Parser)]
#[command(version, about = "The Please Behave ledger")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Apply the embedded database migrations and exit
    Migrate,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt().json().init();
    match Cli::parse().command {
        Some(Command::Migrate) => migrate().await,
        None => serve().await,
    }
}

async fn migrate() -> Result<()> {
    let pool = database::connect(&config::database_url()?).await?;
    let applied = database::migrate_with_report(&pool).await?;
    if applied.is_empty() {
        tracing::info!("the schema is already up to date");
    }
    for migration in applied {
        tracing::info!(
            version = migration.version,
            name = %migration.description,
            "applied migration"
        );
    }
    tracing::info!("the migrations are applied");
    Ok(())
}

async fn serve() -> Result<()> {
    let pool = database::connect_lazily(&config::database_url()?)?;
    let address = config::listen_address()?;
    let overview = config::OverviewConfig::from_env()?;
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot listen on {address}"))?;
    tracing::info!(%address, "the ledger is listening");
    http::run_with_overview_config(listener, pool, overview)
        .await
        .context("the server failed")
}
