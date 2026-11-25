# Rule Set compliance by repository

The ledger serves this read to the cockpit. The compatibility version is in
[api.yaml](api.yaml). The [response schema](rule-set-compliance.schema.json)
defines the entries.

`GET /v0/rule-sets/compliance` takes the required `source`, `path` and `id`
query parameters. URL-encode them as for [registered rules](rule.md).

| Status | Meaning |
| --- | --- |
| `200` | A JSON array with an entry for each repository and bound version, ordered by repository and version text. An identity with no current bindings returns an empty array. |
| `400` | An identity field is missing or invalid, or a query parameter is repeated or unknown. |

The ledger selects the newest attestation containing the `check` execution
point for each repository by receipt time, then by greater attestation ID
when receipt times tie. Only bindings whose execution points contain `check`
in that attestation appear in the response. Repeated bindings of the same
version count once.

A registered bound version is complete when every member has a matching
`PASS` result and no matching result has another status. The ledger matches
results by rule source, path and local ID. `non_passing_members` contains the
registered identity and version of each member with a distinct non-`PASS`
status, in member order and then by status. A missing result is reported as
`NOT_RUN`. Every member counts, including disabled members.

An unregistered bound version has `registered: false`, `complete: false` and
an empty `non_passing_members` list. The read uses current registrations and
changes no stored data.
