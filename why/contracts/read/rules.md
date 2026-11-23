# Registered rules

Producer: the ledger. Consumer: the cockpit. The compatibility version is in
[api.yaml](api.yaml). [rules.schema.json](rules.schema.json) defines the response.

`GET /v0/rules` returns `200` and a JSON array with one summary per rule
identity. Each summary describes the greatest registered version, comparing
the major and then minor components numerically. The display name is that
version's stored name, or its local ID if the name is absent. The ledger
sorts by display name in ascending Unicode code point order after Unicode
simple lowercase mapping. It breaks ties by source, path and ID in ascending
code point order, comparing those fields as stored. An empty page returns an
empty array.

| Query parameter | Meaning |
| --- | --- |
| `limit` | The maximum number of summaries, an integer from 1 to 200. Defaults to 100. |
| `before` | A URL-encoded JSON object containing `source`, `path`, `id`, `version` and `scan` from the last summary. Only summaries strictly after that rule in the documented order are returned. Omit it to start a new scan. |

The four rule fields are strings. The ledger uses the registered version to
recover the display name, so the consumer does not have to send the name back.
The `scan` object contains `through` (the inclusive registration time bound
in integer microseconds since the Unix epoch) and `registrations` (the number
of registrations in that scan). Copy those values unchanged. The response
schema also defines the decoded cursor.

A scan reads the registrations visible on its first page. Later registrations
beyond the time bound do not affect that scan, including new versions that
rename a rule. If another registration becomes visible at or before the bound,
the cursor is invalid and the consumer can start a new scan. The ledger
returns `400` for an invalid limit or cursor, repeated parameters or unknown
parameters. A page shorter than the limit ends the scan.

The read changes nothing. The ledger returns texts as stored. Consumers must
treat them as untrusted data and escape them for the display context. For all
versions of one identity, see [Registered rule versions](rule.md).
