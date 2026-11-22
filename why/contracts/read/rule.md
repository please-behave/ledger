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
original registering commit and the time the ledger first stored it in UTC.
Reviews carry their effective `blocking` value and the registered prompt.
Optional fields are omitted when absent from the registration. The ledger
returns all texts and the knowledge document as stored. Consumers must treat
them as untrusted data and escape them for the display context.

The read changes nothing.
