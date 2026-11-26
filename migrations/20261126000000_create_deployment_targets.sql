create table deployment_targets (
    owner text not null check (owner <> ''),
    name text not null check (name collate "C" ~ '^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$'),
    description text check (char_length(description) between 1 and 512),
    primary key (owner, name)
);
