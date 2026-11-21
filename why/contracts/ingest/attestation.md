# Attestation envelope

Producer: the gateway. Consumer: the ledger. All paths of this API start with `/v0/`; the compatibility version is in [api.yaml](api.yaml). [attestation.schema.json](attestation.schema.json) defines the structure of the envelope. The gateway validates structure and bounds and authenticates the sender; the ledger relies on that and parses the body only into this structure.

## Ingest

`POST /v0/attestations` carries one envelope as a JSON body.

| Status | Meaning |
| --- | --- |
| `201` | The attestation and all of its results are stored. The body is a JSON object with the attestation's `id`. |
| `400` | The body is not a valid envelope, contains an unknown field or violates a database constraint. Nothing is stored. |

Any other failure to store is answered with `500` and stores nothing.

## Storage

The attestation and its results are appended in one transaction. The position of a result in `results` is kept. The envelope is stored as received. Its `id` is a version 7 UUID that the ledger generates on receipt. Identifiers of later attestations are greater than those of earlier ones.

The ledger does not authenticate, does not detect repeated deliveries and does not read attestations through this API.
