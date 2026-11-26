with repositories as (
    select repository, platform, host, owner, name
    from repository_inventory
    where not archived
    union all
    select distinct a.repository, null, null, null, null
    from attestations a
    where not exists (
        select 1 from repository_inventory i where i.repository = a.repository
    )
), overview as (
    select r.repository, r.platform, heartbeat.completed_at,
           to_jsonb(latest_check) as latest_check,
           case when head.commit is not null then
               to_jsonb(head) || jsonb_build_object('direct_evidence', gap.observed_heads_since_evidence = 0)
           end as head,
           gap.first_uncovered_at, gap.observed_heads_since_evidence,
           to_jsonb(release) as latest_release, to_jsonb(deployment) as latest_deployment
    from repositories r
    left join lateral (
        select max(completed_at) as completed_at
        from collector_heartbeats
        where platform = r.platform and host = r.host
          and scope in (r.owner || '/*', r.owner || '/' || r.name)
    ) heartbeat on true
    left join lateral (
        select id, commit_sha as commit, overall, received_at
        from attestations
        where repository = r.repository and execution_points @> array['check']
        order by received_at desc, id desc
        limit 1
    ) latest_check on true
    left join lateral (
        select branch, commit_sha as commit, committed_at, observed_at
        from repository_head_observations
        where (platform, host, owner, name) = (r.platform, r.host, r.owner, r.name)
        order by observed_at desc, branch desc, commit_sha desc
        limit 1
    ) head on true
    left join lateral (
        select h.observed_at, h.branch, h.commit_sha
        from repository_head_observations h
        where (h.platform, h.host, h.owner, h.name) = (r.platform, r.host, r.owner, r.name)
          and exists (
            select 1 from attestations a
            where a.repository = r.repository and a.commit_sha = h.commit_sha
              and a.execution_points @> array['check']
          )
        order by h.observed_at desc, h.branch desc, h.commit_sha desc
        limit 1
    ) evidenced_head on true
    left join lateral (
        select min(observed_at) as first_uncovered_at, count(*) as observed_heads_since_evidence
        from repository_head_observations
        where (platform, host, owner, name) = (r.platform, r.host, r.owner, r.name)
          and (evidenced_head.observed_at is null or (observed_at, branch, commit_sha) >
              (evidenced_head.observed_at, evidenced_head.branch, evidenced_head.commit_sha))
    ) gap on true
    left join lateral (
        select version, commit_sha as commit, received_at
        from release_facts
        where repository = r.repository
        order by received_at desc, version desc
        limit 1
    ) release on true
    left join lateral (
        select sender as owner, target_name as name, received_at
        from deployment_events
        where repository = r.repository
        order by received_at desc, id desc
        limit 1
    ) deployment on true
)
select clock.now, min(overview.completed_at) over () as oldest_sync, to_jsonb(overview) as facts
from (select statement_timestamp() as now) clock
left join overview on true
order by overview.repository, overview.platform
