create table deployment_events (
    id bigint generated always as identity primary key,
    sender text not null,
    target_name text not null,
    repository text not null check (char_length(repository) between 1 and 256),
    version text check (char_length(version) between 1 and 256),
    commit_sha text check (commit_sha collate "C" ~ '^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$'),
    auth_method text not null check (auth_method in ('api_key', 'github_oidc')),
    received_at timestamptz not null default statement_timestamp(),
    check ((version is null) <> (commit_sha is null)),
    foreign key (sender, target_name) references deployment_targets (owner, name)
);
