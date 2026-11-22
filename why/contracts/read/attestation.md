# Attestation

Producer: the ledger. Consumer: the cockpit. All paths of this API start with `/v0/`; the compatibility version is in [api.yaml](api.yaml). [attestation.schema.json](attestation.schema.json) defines the structure of the response.

## Read

`GET /v0/attestations/{id}` carries the attestation's identifier in the path.

| Status | Meaning |
| --- | --- |
| `200` | The body is the attestation: the fields of its summary and a `results` array with all of its results in the order in which they were reported; its `result_count` equals the length of that array. The summary is the one that [Latest attestations](latest.md) returns. |
| `400` | The identifier is not a UUID. |
| `404` | No attestation has the identifier. |

The read changes nothing.
