use crate::registration::RegistrationJson;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema = serde_json::from_str(include_str!(
        "../why/contracts/ingest/target-registration.schema.json"
    ))
    .expect("bundled target registration schema is JSON");
    jsonschema::validator_for(&schema).expect("bundled target registration schema is valid")
});

#[derive(Deserialize)]
pub(crate) struct TargetRequest {
    pub sender: String,
    pub targets: Vec<Target>,
}

#[derive(Deserialize)]
pub(crate) struct Target {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Serialize)]
pub(crate) struct TargetOutcome {
    pub name: String,
    pub outcome: TargetOutcomeKind,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum TargetOutcomeKind {
    Created,
    Unchanged,
}

impl TargetRequest {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let RegistrationJson(document) = serde_json::from_slice(body)?;
        ensure!(VALIDATOR.is_valid(&document), "invalid target registration");
        Ok(serde_json::from_value(document)?)
    }
}
