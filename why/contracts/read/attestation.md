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

## Result verification

Every result has a `verification` object. The ledger looks up the stored rule source, path, local ID and version in the registry and compares the reported digest with the registered digest.

| Verification object | Meaning |
| --- | --- |
| `{"status":"verified"}` | The registration matches the identity and version, and the digests are equal. |
| `{"status":"unverified","reason":"mismatch"}` | The registration matches the identity and version, and the digests differ. |
| `{"status":"unverified","reason":"unregistered"}` | The result lacks an identity, version or digest, or there is no registration for that identity and version. |

The ledger computes verification on each read. A later registration can resolve an earlier result while the attestation and its results stay as reported.
