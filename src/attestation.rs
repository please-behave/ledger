use serde::{Deserialize, Deserializer, de::Error};

// The indexed fields must fit a PostgreSQL B-tree entry even in UTF-8.
const MAX_RULE_SOURCE_CHARS: usize = 256;
const MAX_RULE_PATH_CHARS: usize = 256;
const MAX_RULE_ID_CHARS: usize = 128;
const MAX_RULE_VERSION_CHARS: usize = 64;
const MAX_RULE_DIGEST_CHARS: usize = 128;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub sender: String,
    pub attestation: Attestation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attestation {
    pub schema_version: i32,
    pub repository: String,
    pub commit: String,
    pub execution_points: Vec<String>,
    pub overall: Overall,
    pub results: Vec<RuleResult>,
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
    #[serde(default, deserialize_with = "deserialize_present_string")]
    pub version: Option<String>,
    #[serde(default, deserialize_with = "deserialize_present_string")]
    pub digest: Option<String>,
}

fn deserialize_present_string<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
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
