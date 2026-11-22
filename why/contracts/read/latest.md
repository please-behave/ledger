# Latest attestations

Producer: the ledger. Consumer: the cockpit. All paths of this API start with `/v0/`; the compatibility version is in [api.yaml](api.yaml). [latest.schema.json](latest.schema.json) defines the structure of the response; the summary is defined in [attestation.schema.json](attestation.schema.json).

## Read

`GET /v0/attestations/latest` answers with `200` and a JSON array with one summary for every combination of repository and execution points that has an attestation. The summary is that of the newest attestation of the combination, the one received last, and the one with the greater identifier when two were received at the same instant. The array is ordered by repository and then by execution points, and the order does not change while the stored data does not change. The array is empty when nothing is stored. Besides the counts of results with status `WARN` and `NOT_RUN`, every summary carries `result_count`, the number of results the attestation holds, which is 0 for an attestation without results.

The read changes nothing.
