# Attestation

Producer: the ledger. Consumer: the cockpit. All paths of this API start with `/v0/`; the compatibility version is in [api.yaml](api.yaml). [attestation.schema.json](attestation.schema.json) defines the structure of the response.

## Read

`GET /v0/attestations/{id}` carries the attestation's identifier in the path.

| Status | Meaning |
| --- | --- |
| `200` | The body contains the attestation's summary fields, its `rule_sets` bindings and its `results` in reported order. Its `result_count` equals the length of `results`. The summary is the one that [Latest attestations](latest.md) returns. |
| `400` | The identifier is not a UUID. |
| `404` | No attestation has the identifier. |

The read changes nothing.

## Bound Rule Sets

The `rule_sets` array contains the stored bindings in reported order, including
bindings for unregistered versions. Each binding contains its `source`, `path`,
`id` (the Rule Set's local ID), `version` and `execution_points` as reported.
The array is empty when no bindings were stored.

## Stored rule identity

When a result has a stored identity, the ledger returns it in `rule` with
`source`, `path` and `id` (the rule's local ID). The result's top-level `id`
is the run-scoped rule ID. The ledger includes `rule.version` when stored and
omits the digest. It omits `rule` when no identity was reported. The ledger
returns the values as stored, and the consumer must escape them when using
them in links or displaying them.

## Result verification

Every result has a `verification` object. The ledger looks up the stored rule source, path, local ID and version in the registry and compares the reported digest with the registered digest.

| Verification object | Meaning |
| --- | --- |
| `{"status":"verified"}` | The registration matches the identity and version, and the digests are equal. |
| `{"status":"unverified","reason":"mismatch"}` | The registration matches the identity and version, and the digests differ. |
| `{"status":"unverified","reason":"unregistered"}` | The result lacks an identity, version or digest, or there is no registration for that identity and version. |

The ledger computes verification on each read. A later registration can resolve an earlier result while the attestation and its results stay as reported.

## Registered rule name

A result includes `name` when the registration matching its stored rule source, path, local ID and version has a name. The ledger returns the name as stored, regardless of the digest comparison. It omits the field when there is no matching registration or the registration has no name. The consumer must escape this untrusted text when displaying it.
