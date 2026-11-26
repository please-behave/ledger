# Current deployments

`GET /v0/deployments` returns a JSON array with the current deployment for
each target owner, target name and subject repository in scope. The ledger
selects each current event by greatest receipt time and then greatest event
ID. It pages the selected events in that same descending order. See
[api.yaml](api.yaml) for the compatibility version and
[deployments.schema.json](deployments.schema.json) for the response shape.

Each entry carries its event `id`, target `owner` and `name`, subject
`repository`, authenticated `sender`, `auth_method` and `received_at` in
RFC 3339 UTC. The target owner is the sender. Exactly one of `version` and
`reported_commit` is populated with the reported reference. The other is
`null`. The `commit` field carries the reported commit or the commit from
the release fact for the exact repository and version.

| `state` | `commit` | `evidence` |
| --- | --- | --- |
| `evidence` | Known | The selected attestation's `overall` status and `completed_rule_sets`, using the [commit evidence](commit-evidence.md) rules and the [release list](releases.md) summary shape. |
| `no_evidence` | Known | `null`. There is no qualifying attestation for the repository and commit. |
| `release_not_recorded` | `null` | `null`. There is no release fact for the reported version. |

The ledger resolves release facts and commit evidence at read time. A late
release fact or attestation is reflected on a subsequent read. The read
leaves stored data unchanged. Consumers must escape stored text for display
and present deployments and evidence as reported by their senders.

| Query parameter | Meaning |
| --- | --- |
| `repository` | Optional URL-encoded repository, matched as stored, with the constraints of the [deployment fact](../ingest/deployment-fact.md). |
| `limit` | Page size from 1 to 200. Defaults to 100. |
| `before` | Exclusive cursor. Pass the last entry's decimal event ID, keeping the same repository filter. The cursor event must exist in scope and may have been superseded. |

The response is `200` with an empty array when nothing matches or the page
is exhausted. It is `400` for an invalid, unknown or repeated parameter, or
if the cursor does not identify an event in scope. Each page reflects the
current events at the time of the read.
