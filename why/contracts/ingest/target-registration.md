# Deployment target registration

The gateway sends the authenticated sender and its targets to
`POST /v0/target-registrations`. The [request schema](target-registration.schema.json)
defines the JSON body. The body limit is 2 MiB (2,097,152 bytes). Text lengths
count Unicode characters. The ledger rejects duplicate JSON keys and validates
the whole batch before writing.

The sender is the owner of each target. The gateway supplies the normalized,
lowercase repository identity for GitHub OIDC and the key label for API key
authentication (why:DEC-0015, why:DEC-0022). The ledger uses the supplied sender
and target name exactly as provided for the key.

A valid batch returns `200 OK` with an `outcomes` array in request order. Each
entry has a `name` and an `outcome` of `created` or `unchanged`.

| Outcome | Meaning |
|---|---|
| `created` | The owner and name were new. The ledger stored the target with its optional description. |
| `unchanged` | The owner and name were already registered. The ledger preserved the first registration, including its description. |

If a new name occurs more than once in the batch, the first entry creates it
and subsequent entries are unchanged. The same name under another owner
identifies a separate target. The ledger writes the batch atomically.

Invalid requests return `400`. Requests above the body limit return `413`.
Unexpected storage failures return `500`.
