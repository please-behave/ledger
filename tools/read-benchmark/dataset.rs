use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

pub const REPOSITORIES: u32 = 1_000;
pub const ATTESTATIONS: u32 = 20_000;
pub const OWNER: &str = "benchmark";

pub fn epoch() -> DateTime<Utc> {
    DateTime::from_timestamp(1_767_225_600, 0).expect("benchmark epoch is valid")
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Dataset {
    pub repositories: Vec<Repository>,
    pub attestations: Vec<Attestation>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Repository {
    pub name: String,
    pub head: String,
    pub profile: Profile,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Current,
    Failing,
    Overdue,
    NeverIntegrated,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct Attestation {
    pub repository: String,
    pub id: String,
    pub commit: String,
    pub received_at: DateTime<Utc>,
    pub overall: &'static str,
    pub version: String,
    pub target: &'static str,
    pub rule_group: u32,
}

impl Dataset {
    pub fn generate(seed: u64, repositories: u32, attestations: u32) -> Self {
        assert!(repositories > 0 && attestations > 0);
        let mut ranking: Vec<_> = (0..repositories)
            .map(|number| (hash(seed, "rank", number), number))
            .collect();
        ranking.sort_unstable();

        let active = repositories
            .saturating_sub(repositories / 20)
            .min(attestations);
        let mut data = Self {
            repositories: ranking
                .iter()
                .enumerate()
                .map(|(rank, (_, number))| Repository {
                    name: format!("repo-{number:06}"),
                    head: hex(&hash(seed, "head", *number)),
                    profile: match rank as u32 {
                        rank if rank >= active => Profile::NeverIntegrated,
                        rank if (1..=active / 10).contains(&rank) => Profile::Failing,
                        rank if (active / 10 + 1..=active / 5).contains(&rank) => Profile::Overdue,
                        _ => Profile::Current,
                    },
                })
                .collect(),
            attestations: Vec::with_capacity(attestations as usize),
        };

        for number in 0..attestations {
            let rank = number % active;
            let sequence = number / active;
            let repository = &mut data.repositories[rank as usize];
            let received_at =
                epoch() - TimeDelta::days(3) - TimeDelta::minutes(i64::from(attestations - number));
            let id = ((received_at.timestamp_millis() as u128) << 80)
                | (0x7000 << 64)
                | (0x8000 << 48)
                | u128::from(number);
            let commit = hex(&hash(seed, "commit", number));
            if repository.profile != Profile::Overdue {
                repository.head.clone_from(&commit);
            }
            data.attestations.push(Attestation {
                repository: repository.name.clone(),
                id: Uuid::from_u128(id).to_string(),
                commit,
                received_at,
                overall: if repository.profile == Profile::Failing {
                    "FAIL"
                } else {
                    "PASS"
                },
                version: format!("v1.{sequence}.0"),
                target: ["staging", "production", "canary"][sequence as usize % 3],
                rule_group: 1 + rank % 119,
            });
        }

        data
    }
}

fn hash(seed: u64, domain: &str, number: u32) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(seed.to_be_bytes());
    hash.update(domain.as_bytes());
    hash.update(number.to_be_bytes());
    hash.finalize().into()
}

fn hex(bytes: &[u8; 32]) -> String {
    format!("{:x}", sha2::digest::Output::<Sha256>::from(*bytes))
}

impl Attestation {
    pub fn repository_identity(&self) -> String {
        format!("github.com/{OWNER}/{}", self.repository)
    }
}
