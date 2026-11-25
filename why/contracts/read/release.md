# Release detail

`GET /v0/releases/detail?repository=<repository>&version=<version>` takes
the URL-encoded repository and opaque version. Both are matched as stored
and have the same constraints as the [release fact](../ingest/release-fact.md).
The compatibility version is in [api.yaml](api.yaml), and the
[response schema](release.schema.json) defines the release object.

| Status | Meaning |
| --- | --- |
| `200` | The stored release and its commit evidence. |
| `400` | A parameter is missing, repeated, unknown or invalid. |
| `404` | The repository and version do not identify a recorded release. |

The response has the same release metadata as the [release history](releases.md).
Its `evidence` field contains the full [commit evidence](commit-evidence.md)
for the stored repository and commit, or `null` when no attestation qualifies.
A later attestation can change the evidence on a subsequent read without
changing the stored release. The read leaves stored data unchanged.
