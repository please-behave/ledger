use serde::{Deserialize, Deserializer, de::Error};
use serde_json::Value;
use std::sync::LazyLock;

// The indexed fields must fit a PostgreSQL B-tree entry even in UTF-8.
const MAX_RULE_SOURCE_CHARS: usize = 256;
const MAX_RULE_PATH_CHARS: usize = 256;
const MAX_RULE_ID_CHARS: usize = 128;
const MAX_RULE_VERSION_CHARS: usize = 64;
const MAX_RULE_DIGEST_CHARS: usize = 128;

static BOUND_RULE_SETS_VALIDATOR: LazyLock<jsonschema::Validator> = LazyLock::new(|| {
    let schema: Value = serde_json::from_str(include_str!(
        "../why/contracts/ingest/attestation.schema.json"
    ))
    .expect("bundled attestation schema is JSON");
    jsonschema::draft202012::new(&schema["$defs"]["boundRuleSets"])
        .expect("bundled Rule Set binding schema is valid")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub sender: String,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub auth_method: Option<AuthMethod>,
    pub attestation: Attestation,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethod {
    ApiKey,
    GithubOidc,
}

impl AuthMethod {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::GithubOidc => "github_oidc",
        }
    }
}

pub(crate) fn valid_commit(commit: &str) -> bool {
    matches!(commit.len(), 40 | 64) && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attestation {
    pub schema_version: i32,
    pub repository: String,
    pub commit: String,
    pub execution_points: Vec<String>,
    #[serde(
        default = "empty_rule_sets",
        deserialize_with = "deserialize_rule_sets"
    )]
    pub rule_sets: Value,
    pub overall: Overall,
    pub results: Vec<RuleResult>,
}

fn empty_rule_sets() -> Value {
    Value::Array(Vec::new())
}

fn deserialize_rule_sets<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Value, D::Error> {
    let rule_sets = Value::deserialize(deserializer)?;
    if BOUND_RULE_SETS_VALIDATOR.is_valid(&rule_sets) {
        Ok(rule_sets)
    } else {
        Err(D::Error::custom("invalid bound Rule Sets"))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleResult {
    pub id: String,
    pub kind: Kind,
    pub status: Status,
    #[serde(default, deserialize_with = "deserialize_rule")]
    pub rule: Option<RuleMetadata>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleMetadata {
    pub source: String,
    pub path: String,
    pub id: String,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub version: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present")]
    pub digest: Option<String>,
}

fn deserialize_present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

fn deserialize_rule<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<RuleMetadata>, D::Error> {
    let rule = RuleMetadata::deserialize(deserializer)?;
    let valid_version_and_digest = match (&rule.version, &rule.digest) {
        (None, None) => true,
        (Some(version), Some(digest)) => {
            valid_version(version) && bounded_text(digest, MAX_RULE_DIGEST_CHARS)
        }
        _ => false,
    };
    if valid_rule_identity(&rule.source, &rule.path, &rule.id) && valid_version_and_digest {
        Ok(Some(rule))
    } else {
        Err(D::Error::custom("invalid rule metadata"))
    }
}

pub(crate) fn valid_rule_identity(source: &str, path: &str, id: &str) -> bool {
    bounded_text(source, MAX_RULE_SOURCE_CHARS)
        && bounded_text(path, MAX_RULE_PATH_CHARS)
        && bounded_text(id, MAX_RULE_ID_CHARS)
}

fn bounded_text(value: &str, max_chars: usize) -> bool {
    !value.is_empty() && value.chars().nth(max_chars).is_none() && !value.contains('\0')
}

pub(crate) fn valid_version(version: &str) -> bool {
    let valid_component = |part: &str| {
        !part.is_empty()
            && (part == "0" || !part.starts_with('0'))
            && part.bytes().all(|byte| byte.is_ascii_digit())
    };
    version.len() <= MAX_RULE_VERSION_CHARS
        && version
            .split_once('.')
            .is_some_and(|(major, minor)| valid_component(major) && valid_component(minor))
}

#[derive(Clone, Copy, Deserialize)]
pub enum Overall {
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "ERROR")]
    Error,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Check,
    Review,
    Integrity,
}

#[derive(Clone, Copy, Deserialize)]
pub enum Status {
    #[serde(rename = "PASS")]
    Pass,
    #[serde(rename = "FAIL")]
    Fail,
    #[serde(rename = "ERROR")]
    Error,
    #[serde(rename = "NOT_RUN")]
    NotRun,
    #[serde(rename = "WARN")]
    Warn,
}

impl Overall {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Error => "ERROR",
        }
    }
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Review => "review",
            Self::Integrity => "integrity",
        }
    }
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "PASS",
            Self::Fail => "FAIL",
            Self::Error => "ERROR",
            Self::NotRun => "NOT_RUN",
            Self::Warn => "WARN",
        }
    }
}
