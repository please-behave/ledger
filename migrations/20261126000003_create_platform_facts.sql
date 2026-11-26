create table repository_inventory (
    platform text not null check (platform <> ''),
    host text not null check (host <> '' and position('/' in host) = 0),
    owner text not null check (owner <> '' and position('/' in owner) = 0),
    name text not null check (name <> '' and position('/' in name) = 0),
    repository text generated always as (host || '/' || owner || '/' || name) stored,
    sender text not null check (sender <> ''),
    default_branch text not null check (default_branch <> ''),
    archived boolean not null,
    fork boolean not null,
    first_seen_at timestamptz not null,
    last_seen_at timestamptz not null,
    last_activity_at timestamptz,
    gone_at timestamptz,
    gone_sender text check (gone_sender <> ''),
    check (first_seen_at <= last_seen_at),
    check (gone_at > last_seen_at),
    check ((gone_at is null) = (gone_sender is null)),
    primary key (platform, host, owner, name)
);

create table repository_head_observations (
    platform text not null check (platform <> ''),
    host text not null check (host <> '' and position('/' in host) = 0),
    owner text not null check (owner <> '' and position('/' in owner) = 0),
    name text not null check (name <> '' and position('/' in name) = 0),
    repository text generated always as (host || '/' || owner || '/' || name) stored,
    sender text not null check (sender <> ''),
    branch text not null check (branch <> ''),
    commit_sha text not null check (commit_sha collate "C" ~ '^([0-9a-f]{40}|[0-9a-f]{64})$'),
    committed_at timestamptz not null,
    observed_at timestamptz not null,
    received_at timestamptz not null default statement_timestamp(),
    primary key (platform, host, owner, name, branch, observed_at, commit_sha)
);

create table collector_heartbeats (
    sender text not null check (sender <> ''),
    platform text not null check (platform <> ''),
    host text not null check (host <> '' and position('/' in host) = 0),
    scope text not null check (scope ~ '^[^/]+/[^/]+$'),
    completed_at timestamptz not null,
    primary key (sender, platform, host, scope)
);
