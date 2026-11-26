-- no-transaction
create index concurrently deployment_events_current_idx
    on deployment_events (sender, target_name, repository, received_at desc, id desc);
