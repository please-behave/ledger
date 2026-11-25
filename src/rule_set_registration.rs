use crate::registration::{Outcome, RegistrationJson};
use anyhow::{Result, ensure};
use axum::http::uri::Authority;
use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use std::sync::LazyLock;

static VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema = serde_json::from_str(include_str!(
        "../why/contracts/ingest/rule-set-registration.schema.json"
    ))
    .expect("bundled Rule Set registration schema is JSON");
    jsonschema::validator_for(&schema).expect("bundled Rule Set registration schema is valid")
});

#[derive(Deserialize)]
pub(crate) struct RuleSetRequest {
    pub sender: String,
    pub commit: String,
    pub sets: Vec<RuleSet>,
}

#[derive(Deserialize)]
pub(crate) struct RuleSet {
    pub source: String,
    pub path: String,
    pub id: String,
    pub version: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub knowledge: Option<Box<RawValue>>,
    pub members: Vec<Member>,
}

// Field order defines the member-list digest (CON-0009).
#[derive(Deserialize, Serialize)]
pub(crate) struct Member {
    source: String,
    path: String,
    id: String,
    version: String,
    digest: String,
    enabled: bool,
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub(crate) enum SetOutcome {
    InvalidMembers {
        failures: Vec<MemberFailure>,
    },
    #[serde(untagged)]
    Registered(Outcome),
}

#[derive(Serialize, sqlx::FromRow)]
pub(crate) struct MemberFailure {
    member_index: i64,
    reason: FailureReason,
}

#[derive(Serialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
enum FailureReason {
    Unregistered,
    DigestMismatch,
}

impl RuleSetRequest {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let RegistrationJson(document) = serde_json::from_slice(body)?;
        ensure!(
            VALIDATOR.is_valid(&document),
            "invalid Rule Set registration"
        );
        // Decode the original bytes so the knowledge document can be stored verbatim.
        let request: Self = serde_json::from_slice(body)?;
        ensure!(
            request.sets.iter().all(|set| {
                valid_source(&set.source)
                    && set
                        .members
                        .iter()
                        .all(|member| valid_source(&member.source))
            }),
            "invalid Git source"
        );
        Ok(request)
    }
}

fn valid_source(source: &str) -> bool {
    source.split_once('/').is_some_and(|(host, _)| {
        host.parse::<Authority>()
            .is_ok_and(|authority| authority.host() == host)
    })
}
