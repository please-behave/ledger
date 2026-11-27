# Commit evidence

The ledger serves reported evidence for an exact repository and commit.
The compatibility version is in [api.yaml](api.yaml), and the
[response schema](commit-evidence.schema.json) defines the response.

`GET /v0/repositories/commit-evidence` takes the required `repository` and
`commit` query parameters. URL-encode the stored repository identity and
supply the full 40- or 64-digit hexadecimal commit ID. Both values are
matched as stored.

| Status | Meaning |
| --- | --- |
| `200` | The `evidence` field contains the selected attestation's metadata and Rule Set completeness, or is `null` when no attestation qualifies. |
| `400` | A parameter is missing, repeated or unknown, the commit ID is malformed, or the repository contains a NUL character. |

The ledger selects the newest attestation for the pair whose execution points
contain `check`, ordered by receipt time and then by greater attestation ID.
The evidence includes that attestation's `id`, `overall`, `received_at`,
`sender` and `auth_method`. The timestamp follows
[why:DEC-0027](https://github.com/please-behave/why/blob/main/why/decisions/DEC-0027.yaml).
The method is `null` when the stored envelope did not carry it. Consumers can use the ID to
read the [attestation detail](attestation.md) and its reported rule results.

Only bindings at `check` in the selected attestation appear in `rule_sets`.
Repeated bindings of the same version count once. The list is ordered by
source, path, ID and version text. Each set includes its registered display
name, falling back to its local ID. Registration, completeness and
`non_passing_members` follow the [repository compliance contract](rule-set-compliance.md).
An attestation with no bindings at `check` has an empty `rule_sets` list.

The read changes no stored data. Consumers must escape stored text for display
and present the result as evidence reported by the sender.
