set local lock_timeout = '1s';

alter table attestation_results
    add column rule_source text,
    add column rule_path text,
    add column rule_local_id text,
    add column rule_version text,
    add column rule_digest text;
