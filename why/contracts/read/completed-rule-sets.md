# Completed Rule Sets by repository

The ledger serves this read to the cockpit. The compatibility version is in
[api.yaml](api.yaml). The [response schema](completed-rule-sets.schema.json)
defines the entries.

`GET /v0/repositories/completed-rule-sets` returns `200` and a JSON array
ordered by repository. Each repository has a `sets` list containing its
completed Rule Sets under the [compliance contract](rule-set-compliance.md),
ordered by source, path, ID and version text. Each set carries its bound
identity, version and display name from that version's registration, with the
set ID as the fallback name. Repositories with no completed set are omitted.

Consumers must treat stored text as untrusted and escape it for display.
The read changes no stored data.
