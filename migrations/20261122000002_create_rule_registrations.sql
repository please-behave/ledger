create table rule_registrations (
    source text not null check (char_length(source) between 1 and 256),
    path text not null check (char_length(path) between 1 and 256),
    id text not null check (char_length(id) between 1 and 128),
    version text not null check (
        char_length(version) <= 64 and version ~ '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
    ),
    kind text not null,
    name text,
    description text,
    blocking boolean,
    prompt text,
    knowledge json,
    digest text not null,
    sender text not null,
    commit_sha text not null check (commit_sha ~ '^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$'),
    registered_at timestamptz not null default now(),
    primary key (source, path, id, version),
    check (
        (kind = 'check' and blocking is null and prompt is null) or
        (kind = 'review' and blocking is not null and prompt is not null)
    )
);
