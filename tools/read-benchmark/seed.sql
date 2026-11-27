create temporary table fixture_runs on commit drop as
select r.*, 'github.com/benchmark/' || r.repository as identity
from fixture
cross join lateral jsonb_to_recordset(data->'attestations') as r(
    repository text, id uuid, commit text, received_at timestamptz,
    overall text, version text, target text, rule_group integer
);

create temporary table fixture_repositories on commit drop as
select r.* from fixture
cross join lateral jsonb_to_recordset(data->'repositories') as r(name text, head text, profile text);

insert into repository_inventory
    (platform, host, owner, name, sender, default_branch, archived, fork,
     first_seen_at, last_seen_at, last_activity_at)
select 'github', 'github.com', 'benchmark', name, 'benchmark', 'main', false, false,
       epoch - interval '1 year', epoch, epoch - interval '1 day'
from fixture_repositories cross join fixture;

insert into collector_heartbeats (sender, platform, host, scope, completed_at)
select 'benchmark', 'github', 'github.com', 'benchmark/*', epoch from fixture;

insert into repository_head_observations
    (platform, host, owner, name, sender, branch, commit_sha, committed_at, observed_at, received_at)
select 'github', 'github.com', 'benchmark', repository, 'benchmark', 'main', commit,
       received_at - interval '2 seconds', received_at - interval '1 second', received_at
from fixture_runs
union all
select 'github', 'github.com', 'benchmark', name, 'benchmark', 'main', head,
       epoch - interval '2 days', epoch - interval '2 days', epoch - interval '2 days'
from fixture_repositories cross join fixture
where profile in ('overdue', 'never_integrated');

insert into rule_registrations
    (source, path, id, version, kind, name, description, digest, sender, commit_sha, registered_at)
select 'github.com/benchmark/rules', 'checks', 'check-' || n, '1.0', 'check',
       'Benchmark check ' || lpad(n::text, 3, '0'), 'Synthetic check for the read benchmark.',
       'v1:sha256:' || encode(sha256(('check-' || n)::bytea), 'hex'),
       'benchmark', repeat('a', 64), epoch - interval '1 year'
from generate_series(0, 239) n cross join fixture;

insert into rule_set_registrations
    (source, path, id, version, name, description, members, digest, sender, commit_sha, registered_at)
select 'github.com/benchmark/rules', 'sets', 'set-' || n, '1.0',
       'Benchmark set ' || lpad(n::text, 3, '0'), 'Synthetic Rule Set for the read benchmark.',
       (select json_agg(json_build_object(
           'source', source, 'path', path, 'id', id, 'version', version, 'enabled', true
       ) order by id)
        from rule_registrations where id in ('check-' || (2 * n), 'check-' || (2 * n + 1))),
       'v1:sha256:' || encode(sha256(('set-' || n)::bytea), 'hex'),
       'benchmark', repeat('a', 64), epoch - interval '1 year'
from generate_series(0, 119) n cross join fixture;

create temporary table fixture_results on commit drop as
select a.id as attestation_id, m.position::integer - 1 as position, r.*,
       case when m.position = 1 then a.overall else 'PASS' end as status
from fixture_runs a
cross join lateral unnest(array[0, 1, 2 * a.rule_group, 2 * a.rule_group + 1])
    with ordinality as m(rule_number, position)
join rule_registrations r on r.id = 'check-' || m.rule_number;

create temporary table fixture_documents on commit drop as
select attestation_id, jsonb_agg(jsonb_build_object(
    'id', id, 'kind', kind, 'status', status,
    'rule', jsonb_build_object(
        'source', source, 'path', path, 'id', id, 'version', version, 'digest', digest
    )
) order by position) as results
from fixture_results group by attestation_id;

insert into attestations
    (id, received_at, sender, repository, commit_sha, execution_points, overall,
     schema_version, document, rule_sets)
select a.id, a.received_at, 'benchmark', a.identity, a.commit, array['check'], a.overall, 1,
       jsonb_build_object('sender', 'benchmark', 'auth_method', 'github_oidc',
           'attestation', jsonb_build_object(
               'schema_version', 1, 'repository', a.identity, 'commit', a.commit,
               'execution_points', array['check'], 'overall', a.overall,
               'results', documents.results, 'rule_sets', bindings.value
           )), bindings.value
from fixture_runs a
join fixture_documents documents on documents.attestation_id = a.id
cross join lateral (
    select jsonb_agg(jsonb_build_object(
        'source', source, 'path', path, 'id', id, 'version', version,
        'execution_points', array['check']
    ) order by id) as value
    from rule_set_registrations where id in ('set-0', 'set-' || a.rule_group)
) bindings;

insert into attestation_results
    (attestation_id, position, rule_id, kind, status, rule_source,
     rule_path, rule_local_id, rule_version, rule_digest)
select attestation_id, position, id, kind, status, source, path, id, version, digest
from fixture_results;

insert into release_facts (repository, version, commit_sha, sender, auth_method, received_at)
select identity, version, commit, 'benchmark', 'github_oidc', received_at + interval '1 second'
from fixture_runs;

insert into deployment_targets (owner, name, description)
select 'benchmark', target, 'Synthetic deployment target'
from unnest(array['staging', 'production', 'canary']) target;

insert into deployment_events
    (sender, target_name, repository, version, commit_sha, auth_method, received_at)
select 'benchmark', target, identity,
       case when target <> 'canary' then version end,
       case when target = 'canary' then commit end,
       'github_oidc', received_at + interval '2 seconds'
from fixture_runs order by received_at;
