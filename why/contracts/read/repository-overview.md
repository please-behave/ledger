# Repository overview

`GET /v0/repositories/overview` returns `200` with a heartbeat summary
and all repository entries from one read. The compatibility version
is in [api.yaml](api.yaml) and the
[response schema](repository-overview.schema.json) defines the shape.
Entries are ordered by ascending repository identity, then platform.

Each non-archived inventory row contributes an entry with its `repository`
key (`host/owner/name`), `platform` and `managed: true`. This includes
retained inventory rows for repositories marked gone. A repository key
reported in an attestation that is absent from inventory contributes
one entry with `managed: false` and `platform: null`. An archived
inventory row never appears and still prevents an unmanaged entry for
its key (why:DEC-0025, [Why #28](https://github.com/please-behave/why/issues/28)).
Reports join on the repository key as stored.

## Heartbeat and status

Each entry's `heartbeat_completed_at` is the completion time of the
latest heartbeat covering its platform, host and owner-wide or exact
repository scope. The field is `null` when no heartbeat covers the entry.
An unmanaged entry has no qualified scope. An unrelated heartbeat cannot
refresh a repository's status.

The top-level `heartbeat.completed_at` is the oldest non-null covering
completion time among the returned entries, or `null` when none exists.
Its `stale` flag is true when that time is absent or older than the
configured threshold. Each entry applies the threshold to its own
covering heartbeat.

Apply the status ladder in this order (why:DEC-0024).

| State | Token | Condition |
| --- | --- | --- |
| `unknown` | `status-not-run` | No covering heartbeat or the covering heartbeat is older than the staleness threshold. |
| `never_integrated` | `status-failed` | No attestation contains `check`. |
| `unknown` | `status-not-run` | No head has been observed. |
| `current` | `status-passed` | The latest observed head has direct check evidence. |
| `pending` | `status-allowed` | The head lacks direct evidence and the grace period has not elapsed. |
| `overdue` | `status-warning` | The head lacks direct evidence and the grace period has elapsed. |

The head is the latest stored default-branch observation, ordered by
descending observation time, then descending branch and commit. Its
`direct_evidence` flag uses the exact repository and commit match of
[commit evidence](commit-evidence.md), independently of the verdict.
The summary `latest_check` selects the newest attestation containing
`check` by receipt time and then identifier, across all commits.
`last_evidence_at` is the receipt time of that attestation.

`observed_heads_since_evidence` counts observations after the newest
observation with direct evidence, or all observations when none has
direct evidence. It is a lower bound based on sampled heads. When
check evidence exists but the latest head lacks it, `pending_since`
is the first of those observations and `pending_age_seconds` is the
elapsed whole seconds since then. These fields are available even
when the heartbeat is stale. They are `null` if no check evidence
exists or there are no observations after the last evidenced head.

`overdue_step` and `overdue_steps` are `null` outside `overdue`.
`overdue_step` is the one-based index of the last configured step start
reached after the grace period. Step 1 starts at zero seconds overdue.
`overdue_steps` is the number of configured step starts.
See the [configuration defaults](../../../README.md#running).

## Latest reports

`latest_release` selects the newest recorded release by receipt time,
then descending version. `latest_deployment` selects the event with
this repository as the subject by receipt time, then descending event
ID. Its owner and name identify the deployment target. A summary is
`null` when no applicable fact exists. The same applies to missing
`latest_check` and `head` values.

The read leaves all stored data unchanged. Timestamps are RFC 3339 UTC.
Consumers must escape stored text for display and present attestations,
releases and deployments as reported by their senders.
