# Release history

`GET /v0/releases?repository=<repository>` returns recorded releases for the
specified repository. The compatibility version is in [api.yaml](api.yaml).
The [response schema](releases.schema.json) defines the list entries and cursor.

Both release lists return a JSON array ordered by descending receipt time.
Ties are broken by descending repository and version text. Versions are
opaque, so their text only determines order when receipt times tie. Each
entry includes the repository, version, commit, authenticated sender,
authentication method and receipt time in RFC 3339 UTC.

The `evidence` field is `null` when no attestation qualifies. Otherwise it
contains the evidence attestation's `overall` status and the
`completed_rule_sets` for that attestation, ordered by source, path, ID and
version text. The sets carry their identity, version and display name.
Evidence is computed at read time using the [commit evidence](commit-evidence.md)
selection and completeness rules. Evidence for other commits or repositories
does not contribute.

Both lists accept `limit` from 1 to 200, defaulting to 100, and an exclusive
`before` cursor. For the next page, URL-encode a JSON object with the last
entry's `repository` and `version`. The ledger uses the stored receipt time
of that release for the cursor. A history cursor must belong to the
requested repository. Omit `repository` on the latest-releases read.

| Status | Meaning |
| --- | --- |
| `200` | A page of release summaries. An unknown repository or exhausted page yields an empty array. |
| `400` | A parameter is missing, repeated, unknown or invalid, or the cursor does not identify a recorded release in scope. |

URL-encode repository and version values. Both are matched as stored and
have the same constraints as the [release fact](../ingest/release-fact.md).
The reads leave stored data unchanged. Consumers must escape stored text
for display and present evidence as reported by the sender.
