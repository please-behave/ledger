-- no-transaction
create index concurrently attestation_results_rule_identity_version_idx
    on attestation_results (rule_source, rule_path, rule_local_id, rule_version)
    where rule_version is not null;
