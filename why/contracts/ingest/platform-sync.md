# Platform sync

Collectors send a completed sync to the ledger with
`POST /v0/facts/platform-syncs` on the internal network. The ledger owns
this contract. The first producer is `github-collector`. The gateway must
not forward this route. The ledger trusts the collector's sender label
inside the private network (why:DEC-0007, why:DEC-0024).

The `/v0/` prefix declares the experimental compatibility version in
[api.yaml](api.yaml). The [request schema](platform-sync.schema.json)
defines the JSON body. The body limit is 16 MiB (16,777,216 bytes).
The ledger rejects duplicate JSON keys.

## Scope and identity

One request reports one platform, host and scope. A scope is `owner/*`
for every repository of that owner, or `owner/name` for one repository.
A collector with multiple scopes submits a complete snapshot for each
scope. Omit a submission when the platform read for that scope failed.
An empty inventory declares that the completed sync found no repositories
in its scope.

The repository key is `host/owner/name`, the normalized Git identity
used by attestations (why:DEC-0010). Submit a host without a scheme, user
information or port and repository names without a transport suffix.
The ledger qualifies this key by platform and lowercases the host.
For `github` it also lowercases the owner and repository name. For other
platforms it preserves their case.

## Inventory, heads and heartbeat

- `inventory` lists every repository found in the scope exactly once,
  with its name, default branch, archive and fork flags, and the
  platform's last-activity time. Use `null` when that time is unavailable.
  The ledger keeps one inventory row per qualified repository, with the
  first and last completion times at which it was seen. A newer sighting
  replaces the repository attributes and records its sender.
  The first completed omission after a sighting marks the row gone at
  that sync's completion time and records the omission's sender. A
  sighting at or after that omission clears it unless a later accepted
  sync already omitted the repository. A sighting takes precedence over
  an omission with the same completion time. For late sightings from
  an overlapping scope, the ledger checks the covering heartbeats it
  still has and preserves any earlier omission already on the row.
- `heads` lists the first or changed default-branch head observed for
  each repository during the sync. A head must name a repository in
  `inventory` and match its default branch. The commit ID is full-length
  hexadecimal and is stored in lowercase. Include its commit time and
  the time of observation, which must be at or before sync completion.
  For each reported head, the ledger finds the stored observation for
  the same qualified repository and branch with the latest strictly
  earlier observation time. If several observations share that earlier
  time, the highest commit ID sorts first. The ledger skips the head if
  the commit matches that predecessor. Otherwise it appends the head
  with its receipt time and sender, including when a branch returns to
  an earlier commit. Retrying a stored observation with the same
  qualified repository, branch, commit and observation time preserves
  the original observation and receipt. Late reports use the same
  comparison and leave stored observations intact. Inserting a late
  observation before a stored observation of the same commit can
  therefore leave consecutive observations of that commit in the history.
- The heartbeat is the sender, qualified scope and `completed_at`.
  The ledger stores only the latest completion for each sender and
  scope. A request with the same or an older completion for that sender
  and scope succeeds as a no-op. Reuse the original completion and
  observation times on retry.

`completed_at`, non-null `last_activity_at`, `committed_at` and
`observed_at` must follow the shared timestamp convention in
[why:DEC-0027](https://github.com/please-behave/why/blob/main/why/decisions/DEC-0027.yaml).
Send RFC 3339 UTC with a literal `Z` suffix and up to six fractional
digits. The ledger rejects numeric offsets, including `+00:00`.

Inventory updates, omissions, heads and the heartbeat commit together.

| Status | Meaning |
| --- | --- |
| `204 No Content` | The sync was recorded or an equal or newer sync from the same sender and scope was already recorded. |
| `400 Bad Request` | The body or facts violate the contract. |
| `413 Payload Too Large` | The body exceeds the size limit. |
| `500 Internal Server Error` | The ledger could not store the sync. |
