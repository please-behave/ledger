alter table attestations
    add column rule_sets jsonb not null default '[]'::jsonb;
