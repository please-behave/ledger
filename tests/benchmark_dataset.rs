#[path = "../tools/read-benchmark/dataset.rs"]
mod dataset;

use dataset::{ATTESTATIONS, Dataset, Profile, REPOSITORIES, epoch};
use std::collections::HashSet;
use uuid::Uuid;

#[test]
fn the_default_benchmark_fixture_is_seeded_and_has_mixed_repository_profiles() {
    let data = Dataset::generate(42, REPOSITORIES, ATTESTATIONS);
    assert_eq!(data, Dataset::generate(42, REPOSITORIES, ATTESTATIONS));
    assert_ne!(data, Dataset::generate(43, REPOSITORIES, ATTESTATIONS));
    assert_eq!(data.repositories.len(), 1_000);
    assert_eq!(data.attestations.len(), 20_000);

    let latest: std::collections::HashMap<_, _> = data
        .attestations
        .iter()
        .map(|row| (row.repository.as_str(), row))
        .collect();
    let mut current = 0;
    let mut failing = 0;
    let mut overdue = 0;
    let mut never_integrated = 0;
    for repository in &data.repositories {
        let attestation = latest.get(repository.name.as_str());
        match repository.profile {
            Profile::Current => {
                let attestation = attestation.unwrap();
                assert_eq!(repository.head, attestation.commit);
                assert_eq!(attestation.overall, "PASS");
                current += 1;
            }
            Profile::Failing => {
                let attestation = attestation.unwrap();
                assert_eq!(repository.head, attestation.commit);
                assert_eq!(attestation.overall, "FAIL");
                failing += 1;
            }
            Profile::Overdue => {
                assert_ne!(repository.head, attestation.unwrap().commit);
                overdue += 1;
            }
            Profile::NeverIntegrated => {
                assert!(attestation.is_none());
                never_integrated += 1;
            }
        }
    }
    assert!(current > data.repositories.len() / 2);
    assert!(failing > 0 && overdue > 0 && never_integrated > 0);

    let targets: HashSet<_> = data.attestations.iter().map(|row| row.target).collect();
    assert_eq!(targets, HashSet::from(["staging", "production", "canary"]));
}

#[test]
fn benchmark_fixture_honors_small_counts_and_preserves_attestation_order() {
    for (repositories, attestations) in [(1, 1), (3, 17), (17, 3)] {
        let data = Dataset::generate(1, repositories, attestations);
        assert_eq!(data.repositories.len(), repositories as usize);
        assert_eq!(data.attestations.len(), attestations as usize);
        assert!(
            data.attestations
                .is_sorted_by(|left, right| left.id < right.id)
        );

        let releases: HashSet<_> = data
            .attestations
            .iter()
            .map(|row| (row.repository_identity(), &row.version))
            .collect();
        assert_eq!(releases.len(), attestations as usize);
        for row in data.attestations {
            assert_eq!(Uuid::parse_str(&row.id).unwrap().get_version_num(), 7);
            assert!(row.received_at < epoch());
            assert!((1..120).contains(&row.rule_group));
        }
    }
}
