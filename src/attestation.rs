use serde::Deserialize;

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
