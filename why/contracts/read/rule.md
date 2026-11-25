# Registered rule versions

Producer: the ledger. Consumer: the cockpit. The compatibility version is in
[api.yaml](api.yaml). [rule.schema.json](rule.schema.json) defines the response.

`GET /v0/rules/detail` takes the three required query parameters `source`,
`path` and `id`. URL-encode each value as a query parameter so characters
such as slashes, ampersands and plus signs survive transport. The identity
fields follow the [registration contract](../ingest/rule-registration.md).

| Status | Meaning |
| --- | --- |
| `200` | A JSON array of all registered versions for the identity, ordered by numeric major and then numeric minor, from smallest to greatest. |
| `400` | An identity field is missing or invalid, or a query parameter is repeated or unknown. |
| `404` | No versions are registered for this identity. |

Each entry contains the registered rule fields, its canonical digest, the
original registering commit, the time the ledger first stored it in UTC and
the `owner`. The owner is the authenticated sender stored from the first
registration of that version, as received. It is a repository identity for
GitHub OIDC or a key label for API keys. A repeat registration with the same
digest preserves the owner.

Each version's `rule_sets` array lists every registered Rule Set version
that contains that exact rule version, including memberships registered as
disabled. Each entry contains the set's `source`, `path`, `id`, `version`
and display `name`, which falls back to the set ID when absent. Entries are
ordered by source, path and ID in ascending code point order, then by numeric
major and minor version. A rule version with no matching Rule Set version
has an empty array.
Reviews carry their effective `blocking` value and the registered prompt.
Optional fields are omitted when absent from the registration. The ledger
returns all texts and the knowledge document as stored. Consumers must treat
them as untrusted data and escape them for the display context.

The read changes nothing.

The `owner` and `rule_sets` fields are additive changes to the `v0` response.
Consumers that ignore additional response fields can continue reading the
same version.
