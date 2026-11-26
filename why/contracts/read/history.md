# Attestation history

Producer: the ledger. Consumer: the cockpit. All paths of this API start with `/v0/`; the compatibility version is in [api.yaml](api.yaml). [history.schema.json](history.schema.json) defines the structure of the response; the summary is defined in [attestation.schema.json](attestation.schema.json) and is the one that [Latest attestations](latest.md) returns.

## Read

`GET /v0/attestations` answers with `200` and a JSON array of summaries ordered by identifier from greatest to smallest, which is from newest to oldest. The array is empty when nothing matches.

| Query parameter | Meaning |
| --- | --- |
| `limit` | The greatest number of summaries to return, an integer from 1 to 200. Defaults to 100. |
| `before` | A UUID. Only attestations with a smaller identifier are returned. Without it the page starts at the newest attestation. The identifier need not belong to a stored attestation. |
| `repository` | A URL-encoded normalized repository identity, for example `github.com%2Fowner%2Frepository`. Only attestations with that exact stored identity match. Omit it to include all repositories. |

The answer is `400` when `limit` is not an integer from 1 to 200, when `before` is not a UUID, when `repository` is empty or contains a NUL character, when a parameter is given more than once or when another parameter is given.

## Paging

A reader pages by passing the identifier of the last summary it received as `before` and keeping the same `repository` filter, if present. The filter is applied before the page limit. A page with fewer summaries than `limit` is the last one. Because identifiers are time-ordered, attestations received between two page reads are never inserted into the pages still to come, and every matching attestation that was stored when the first page was read is returned exactly once.

The read changes nothing.
