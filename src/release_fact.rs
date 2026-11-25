use crate::registration::JsonObject;
use anyhow::{Result, ensure};
use serde::Deserialize;

// Indexed text must fit in a PostgreSQL B-tree entry even in UTF-8.
const MAX_KEY_PART_CHARS: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseFact {
    pub repository: String,
    pub version: String,
    pub commit: String,
    pub sender: String,
    pub auth_method: AuthMethod,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AuthMethod {
    ApiKey,
    GithubOidc,
}

impl AuthMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ApiKey => "api_key",
            Self::GithubOidc => "github_oidc",
        }
    }
}

impl ReleaseFact {
    pub fn parse(body: &[u8]) -> Result<Self> {
        let JsonObject(fact) = serde_json::from_slice::<JsonObject<Self>>(body)?;
        for part in [&fact.repository, &fact.version] {
            ensure!(
                !part.is_empty()
                    && !part.contains('\0')
                    && part.chars().nth(MAX_KEY_PART_CHARS).is_none(),
                "invalid repository or version"
            );
        }
        ensure!(
            matches!(fact.commit.len(), 40 | 64)
                && fact.commit.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid release commit"
        );
        ensure!(
            !fact.sender.is_empty() && !fact.sender.contains('\0'),
            "invalid sender"
        );
        if matches!(fact.auth_method, AuthMethod::GithubOidc) {
            ensure!(
                fact.sender == fact.repository,
                "sender does not own repository"
            );
        }
        Ok(fact)
    }
}
