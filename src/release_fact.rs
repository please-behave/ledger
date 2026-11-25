use crate::attestation::{AuthMethod, valid_commit};
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
        ensure!(valid_commit(&fact.commit), "invalid release commit");
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
