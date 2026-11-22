# Rule registration

The gateway sends the authenticated sender and the registering commit to
`POST /v0/rule-registrations`. The
[request schema](rule-registration.schema.json) defines the JSON body.
The body limit is 2 MiB (2,097,152 bytes), including the envelope. The ledger
rejects duplicate JSON members at every level and validates the entire batch
before writing any registrations.

A rule identifies a normalized Git source, repository-relative rule directory
and source-local ID as defined in why:DEC-0010. Its version follows why:DEC-0011.
A review supplies its effective blocking value and the prompt before parameter
substitution. The ledger keeps the complete knowledge object as received. It
uses the [canonical rule digest](../rule-digest/digest.md) to compute each
outcome's digest. Sender and commit record provenance and are outside the
digest.

A valid batch returns `200` and a JSON object with an `outcomes` array. Each
entry contains `outcome` and `digest`, in the same order as the submitted rules.

| Outcome | Meaning |
|---|---|
| `created` | The source, path, ID and version were new. The ledger stored the registration with its provenance and registration time. |
| `unchanged` | The key already held this digest. The original registration is preserved. |
| `conflict` | The key already held a different digest. The outcome carries the submitted rule's computed digest and the original registration is preserved. |

A conflict has no effect on the other rules. Repetitions within the same batch
are evaluated in request order. A malformed or invalid request returns `400`.
A body above the limit returns `413`. An unexpected storage failure returns
`500`, and completed registrations may already be stored. The gateway can
retry the batch without changing existing registrations.
