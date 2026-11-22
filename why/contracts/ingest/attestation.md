# Attestation envelope

Producer: the gateway. Consumer: the ledger. All paths of this API start with `/v0/`; the compatibility version is in [api.yaml](api.yaml). [attestation.schema.json](attestation.schema.json) defines the structure of the envelope. The gateway authenticates the sender and validates the public attestation. The ledger parses the envelope and validates any rule metadata against this contract.

## Ingest

`POST /v0/attestations` carries one envelope as a JSON body.

| Status | Meaning |
| --- | --- |
| `201` | The attestation and all of its results are stored. The body is a JSON object with the attestation's `id`. |
| `400` | The body is not a valid envelope, contains an unknown field or violates a database constraint. Nothing is stored. |

Any other failure to store is answered with `500` and stores nothing.

## Rule metadata

A result still identifies the rule within the run by its top-level `id`.
It may also include a `rule` object carrying the stable identity defined in
why:DEC-0010. The object requires `source` (normalized Git repository
identity), `path` (repository-relative rule directory) and `id` (rule ID
within that source). The schema limits their lengths.

An object with versioned metadata carries both `version` and `digest`.
The version follows the owner-assigned format in why:DEC-0011 and the length
limit in the CLI's [rule definition](https://github.com/please-behave/cli/blob/main/why/contracts/configuration/rule.md).
The digest is a bounded, non-empty string. An identity alone omits both keys.

## Storage

The attestation and its results are appended in one transaction. The position of a result in `results` is kept. The envelope is stored as received. Its `id` is a version 7 UUID that the ledger generates on receipt. Identifiers of later attestations are greater than those of earlier ones.

The result row stores any supplied rule metadata alongside the run-scoped ID.
Omitted metadata leaves the corresponding columns null.

The ledger does not authenticate, does not detect repeated deliveries and does not read attestations through this API.
