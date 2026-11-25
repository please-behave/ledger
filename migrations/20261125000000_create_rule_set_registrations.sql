create table rule_set_registrations (
    source text not null check (char_length(source) between 1 and 256),
    path text not null check (char_length(path) between 1 and 256),
    id text not null check (char_length(id) between 1 and 128),
    version text not null check (
        char_length(version) <= 64 and version ~ '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$'
    ),
    name text,
    description text,
    knowledge json,
    members json not null check (json_typeof(members) = 'array'),
    digest text not null check (digest ~ '^v1:sha256:[0-9a-f]{64}$'),
    sender text not null,
    commit_sha text not null check (commit_sha ~ '^([0-9a-fA-F]{40}|[0-9a-fA-F]{64})$'),
    registered_at timestamptz not null default now(),
    primary key (source, path, id, version)
);
