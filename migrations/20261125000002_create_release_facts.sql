create table release_facts (
    repository text not null check (char_length(repository) between 1 and 256),
    version text not null check (char_length(version) between 1 and 256),
    commit_sha text not null check (commit_sha ~ '^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$'),
    sender text not null check (sender <> ''),
    auth_method text not null check (auth_method in ('api_key', 'github_oidc')),
    received_at timestamptz not null default now(),
    primary key (repository, version)
);
