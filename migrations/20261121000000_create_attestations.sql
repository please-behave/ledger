create table attestations (
    id uuid primary key,
    received_at timestamptz not null default now(),
    sender text not null,
    repository text not null,
    commit_sha text not null check (commit_sha ~ '^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$'),
    execution_points text[] not null,
    overall text not null check (overall in ('PASS', 'FAIL', 'ERROR')),
    schema_version integer not null,
    document jsonb not null
);

create table attestation_results (
    attestation_id uuid not null references attestations (id),
    position integer not null,
    rule_id text not null,
    kind text not null check (kind in ('check', 'review', 'integrity')),
    status text not null check (status in ('PASS', 'FAIL', 'ERROR', 'NOT_RUN', 'WARN')),
    primary key (attestation_id, position)
);

create index attestations_latest_idx
    on attestations (repository, execution_points, received_at desc, id desc);
create index attestations_repository_commit_idx
    on attestations (repository, commit_sha);
create index attestation_results_rule_id_idx
    on attestation_results (rule_id);
