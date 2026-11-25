# Registered Rule Sets

The ledger serves this read to the cockpit. The compatibility version is in
[api.yaml](api.yaml). The [response schema](rule-sets.schema.json) also defines
the decoded cursor.

`GET /v0/rule-sets` returns `200` and a JSON array with one summary per Rule Set
identity (`source`, `path` and `id`). Each summary describes the greatest
registered `version`, comparing major and then minor numerically. Its `name`
is the stored name or the local ID when the name is absent. The ledger omits
`description` when absent. The `member_count` includes every member of that
version, including disabled members.

The ledger uses the [registered rules paging conventions](rules.md).
It applies the same display-name and identity ordering, the same `limit`
and `before` parameters, and the same validation and scan rules to the
Rule Set registry. Build the cursor from the last Rule Set summary, including
its `scan` object. The scan counts registered Rule Set versions.

The ledger returns texts as stored and leaves registrations unchanged.
Consumers must treat the texts as untrusted and escape them for display.
See [Registered Rule Set versions](rule-set.md) for the history of an identity.
