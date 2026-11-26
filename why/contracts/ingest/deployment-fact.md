# Deployment fact

The gateway sends one JSON object to `POST /v0/facts/deployments`. The
[request schema](deployment-fact.schema.json) defines the body. The body
limit is 2 MiB (2,097,152 bytes). The ledger rejects duplicate JSON keys.

The gateway supplies the authenticated sender and authentication method.
The sender is the lowercase repository identity for GitHub OIDC or the
key label for API key authentication. The pair of sender and target name
must identify a registered target. The subject is the deployed repository
and uses the normalized Git identity of why:DEC-0010. It may belong to any
repository, regardless of who owns the target (why:DEC-0022, why:DEC-0023).

The ledger compares each report with the most recent event for that
owner, target name and subject repository. Versions are opaque strings
compared exactly. Explicit Git object IDs are compared case-insensitively.
A version report and an explicit commit report are distinct. Reports can
name a version before its release fact is recorded. A return to an earlier
version or commit after a change creates another event.

The ledger records its own receipt time when it stores an event. The
event preserves the sender's report and its authentication method.

| Status | JSON body | Meaning |
| --- | --- | --- |
| `201 Created` | `{"outcome":"created"}` | The ledger appended an event. |
| `200 OK` | `{"outcome":"unchanged"}` | The report matches the current deployment. The ledger appended no event. |

Invalid submissions and unregistered targets return `400`. Requests above
the body limit return `413`. Unexpected storage failures return `500`.
