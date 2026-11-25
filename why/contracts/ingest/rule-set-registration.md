# Rule Set registration

The gateway sends the authenticated sender, commit and sets to
`POST /v0/rule-set-registrations`. The [request schema](rule-set-registration.schema.json)
defines the JSON body. The body limit is 2 MiB (2,097,152 bytes), including the
envelope. Limits on text lengths count Unicode characters. Each source starts
with a Git host without a port. The ledger rejects duplicate JSON keys at every
level and validates the entire batch before storing any registrations.

A Rule Set version is keyed by source, directory path, ID and version
(why:DEC-0016). The ledger verifies every member, including disabled members,
using its source, directory path, ID and version to find the registered rule
digest. Members can come from any source. The ledger stores the original
membership, the optional name, description and knowledge document, and the
sender, commit and registration time for the first successful registration
(why:DEC-0017). Registered text is untrusted data and is preserved as received,
including line endings.

The member-list digest is `v1:sha256:` followed by the lowercase hexadecimal
SHA-256 of the compact UTF-8 JSON array of members, in submitted list order.
Each member object is serialized with keys in this order: `source`, `path`,
`id`, `version`, `digest`, `enabled`. Strings use their decoded contents.
Escape quotes, backslashes and ASCII control characters in the range U+0000
through U+001F. Use the short JSON escapes where available and lowercase
`\u00xx` escapes for the remaining characters in that range. Write other
characters directly as UTF-8. Boolean values are serialized as `true` or
`false`. Only this array enters the digest.

A valid batch returns `200 OK` with an `outcomes` array in request order.

| Outcome | Meaning |
|---|---|
| `created` | The key was new and the ledger stored the set version. |
| `unchanged` | The key already held the submitted digest. The original registration is preserved. |
| `conflict` | The key held a different digest. The original registration is preserved. |
| `invalid_members` | Member verification failed. The ledger stores nothing for this set and lists all failures. |

The first three outcomes carry the submitted member-list `digest`. The
`invalid_members` outcome instead carries a nonempty `failures` array in member
order. Each entry has a distinct zero-based `member_index` and a `reason` of
`unregistered` or `digest_mismatch`.

Each outcome affects only its set. Repetitions within the batch are evaluated
in request order. An invalid request returns `400`, and a body above the limit
returns `413`. An unexpected storage failure returns `500`. Completed
registrations may already be stored and can be retried safely.
