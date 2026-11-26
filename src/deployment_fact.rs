use crate::attestation::AuthMethod;
use crate::registration::RegistrationJson;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;

static VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema = serde_json::from_str(include_str!(
        "../why/contracts/ingest/deployment-fact.schema.json"
    ))
    .expect("bundled deployment fact schema is JSON");
    jsonschema::validator_for(&schema).expect("bundled deployment fact schema is valid")
});

#[derive(Deserialize)]
pub(crate) struct DeploymentFact {
    pub target: String,
    pub repository: String,
    pub version: Option<String>,
    pub commit: Option<String>,
    pub sender: String,
    pub auth_method: AuthMethod,
}

impl DeploymentFact {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let RegistrationJson(document) = serde_json::from_slice(body)?;
        ensure!(VALIDATOR.is_valid(&document), "invalid deployment fact");
        Ok(serde_json::from_value(document)?)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeploymentOutcome {
    Created,
    Unchanged,
}
