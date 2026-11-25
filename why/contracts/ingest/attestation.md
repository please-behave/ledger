# Attestation envelope

Producer: the gateway. Consumer: the ledger. All paths of this API start with `/v0/`; the compatibility version is in [api.yaml](api.yaml). [attestation.schema.json](attestation.schema.json) defines the structure of the envelope. The gateway authenticates the sender and validates the public attestation. The ledger parses the envelope and validates any rule metadata and bound Rule Sets against this contract.

## Ingest

`POST /v0/attestations` carries one envelope as a JSON body.

| Status | Meaning |
| --- | --- |
| `201` | The attestation, its bound Rule Sets and all of its results are stored. The body is a JSON object with the attestation's `id`. |
| `400` | The body is not a valid envelope, contains an unknown field or violates a database constraint. Nothing is stored. |

Any other failure to store is answered with `500` and stores nothing.

The gateway may include `auth_method` alongside `sender`, with the value
`api_key` or `github_oidc` from its authentication of the sender. The stored
envelope preserves the field when supplied. Omitting it leaves the
authentication method unknown.

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

## Bound Rule Sets

An attestation may include a `rule_sets` array of up to 100 bindings
(why:DEC-0018). Each entry requires `source`, `path`, `id`, `version` and
`execution_points`. The source is a normalized Git repository identity
with a host and no port. The path is a repository-relative directory,
with `.` for the repository root. Source and path are each limited to
256 characters. The ID contains 1 to 128 ASCII letters, digits, underscores
or hyphens. The version uses the same syntax and limit as rule metadata.
Each binding lists 1 to 64 execution points, each a non-empty string of up
to 128 characters without NUL characters.

## Storage

The attestation and its results are appended in one transaction. The position of a result in `results` is kept. The envelope is stored as received. Its `id` is a version 7 UUID that the ledger generates on receipt. Identifiers of later attestations are greater than those of earlier ones.

The result row stores any supplied rule metadata alongside the run-scoped ID.
Omitted metadata leaves the corresponding columns null.

The attestation row stores the supplied binding list in array order, including
all reported execution points and any unregistered versions. An omitted list
is stored as an empty list. The stored envelope retains whether the list
was supplied.

The ledger does not authenticate, does not detect repeated deliveries and does not read attestations through this API.
