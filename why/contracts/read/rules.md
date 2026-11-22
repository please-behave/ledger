# Registered rules

Producer: the ledger. Consumer: the cockpit. The compatibility version is in
[api.yaml](api.yaml). [rules.schema.json](rules.schema.json) defines the response.

`GET /v0/rules` returns `200` and a JSON array with one summary per rule
identity. Each summary describes the greatest registered version, comparing
the major and then minor components numerically. The array is ordered by
source, then path, then ID, from greatest to smallest in the ledger database's
text ordering. An empty page returns an empty array.

| Query parameter | Meaning |
| --- | --- |
| `limit` | The maximum number of summaries, an integer from 1 to 200. Defaults to 100. |
| `before` | A URL-encoded JSON object with the string fields `source`, `path` and `id`. Only identities strictly smaller than this identity are returned. The identity need not be registered. Omit it to start at the greatest identity. |

The ledger answers `400` for an invalid limit or cursor, repeated parameters
or unknown parameters. To continue, serialize the three identity fields of
the last summary as JSON and URL-encode the result as the value of `before`.
A page shorter than the limit ends the scan. Each identity appears at most
once during the scan. A new identity registered between requests can appear
on a later page if it sorts below the cursor.

The read changes nothing. The ledger returns texts as stored. Consumers must
treat them as untrusted data and escape them for the display context. For all
versions of one identity, see [Registered rule versions](rule.md).
