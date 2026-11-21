use anyhow::{Context, Result};
use std::net::SocketAddr;

const DATABASE_URL: &str = "LEDGER_DATABASE_URL";
const LISTEN_ADDRESS: &str = "LEDGER_LISTEN_ADDRESS";
const DEFAULT_LISTEN_ADDRESS: &str = "0.0.0.0:8080";

pub fn database_url() -> Result<String> {
    std::env::var(DATABASE_URL).with_context(|| format!("{DATABASE_URL} must be set"))
}

pub fn listen_address() -> Result<SocketAddr> {
    let value = std::env::var(LISTEN_ADDRESS).unwrap_or_else(|_| DEFAULT_LISTEN_ADDRESS.into());
    value.parse().with_context(|| {
        format!("{LISTEN_ADDRESS} must be an address such as {DEFAULT_LISTEN_ADDRESS}")
    })
}
