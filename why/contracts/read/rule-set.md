# Registered Rule Set versions

The ledger serves this read to the cockpit. The compatibility version is in
[api.yaml](api.yaml). The [response schema](rule-set.schema.json) defines the
version history.

`GET /v0/rule-sets/detail` takes the required `source`, `path` and `id`
query parameters. URL-encode them as for [registered rules](rule.md).
The identity follows the [Rule Set registration contract](../ingest/rule-set-registration.md).

| Status | Meaning |
| --- | --- |
| `200` | A JSON array of every registered version, ordered by numeric major and then numeric minor, from smallest to greatest. |
| `400` | An identity field is missing or invalid, or a query parameter is repeated or unknown. |
| `404` | No versions are registered for this identity. |

Each entry contains the identity, version, stored name, description and
knowledge document, and the full member list in registration order. Each
member carries the rule identity, version, digest and enabled state.
Optional fields are omitted when absent. The entry includes the set
`digest`, original registering `commit` and `registered_at` time. The
timestamp follows
[why:DEC-0027](https://github.com/please-behave/why/blob/main/why/decisions/DEC-0027.yaml).

The ledger returns texts as stored and leaves registrations unchanged.
Consumers must treat the texts as untrusted and escape them for display.
