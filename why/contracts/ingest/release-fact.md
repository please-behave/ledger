# Release fact

The gateway sends one JSON object to the ledger at `POST /v0/facts/releases`.
The [request schema](release-fact.schema.json) defines the body. The body limit
is 2 MiB (2,097,152 bytes). The ledger rejects duplicate keys.

The repository uses the same platform-neutral Git identity as attestations.
The version is an opaque string and is compared exactly, including whitespace
and case. A release may have any version label, with or without a Git tag.
The commit is the full Git object ID supplied in the request. The ledger
preserves its spelling and compares hexadecimal digits case-insensitively.

The gateway authenticates the sender before forwarding the request. For
`github_oidc`, the sender is the normalized, lowercase GitHub repository
identity and must exactly match the reported repository. For `api_key`,
the sender is the authenticated key's label and can report for any repository
(why:DEC-0020).

The ledger stores the first fact per repository and version with its sender,
authentication method and ledger receipt time. Receipt time describes when
the ledger recorded the fact. Recording a release fact requires no attestation
and creates no evidence.

| Status | JSON body | Meaning |
| --- | --- | --- |
| `201 Created` | `{"outcome":"created"}` | The ledger stored the new release fact. |
| `200 OK` | `{"outcome":"unchanged"}` | The same repository and version already reference the same commit. The first record is preserved. |
| `409 Conflict` | `{"outcome":"conflict"}` | The same repository and version already reference a different commit. The first record is preserved. |

Invalid submissions return `400`. Requests above the body limit return
`413`. Unexpected storage failures return `500`.
