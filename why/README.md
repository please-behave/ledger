# Why

This directory holds the Why of the ledger: its requirements, decisions and API contracts. It follows the [Why documentation approach](https://github.com/please-behave/why), whose purpose, principles and shared decisions are not repeated here.

## Artifacts

| Artifact | Purpose |
|---|---|
| **[Requirement](requirements/README.md)** | A binding, verifiable condition the ledger must satisfy. |
| **Decision** | A significant architectural choice with its context and rationale. |
| **[API Contract](contracts/README.md)** | An interface, its compatibility boundary and exact obligations. |

Each kind has a `schema.json` in its folder that defines the permitted structure. Requirements and decisions are one `REQ-NNNN.yaml` or `DEC-NNNN.yaml` file each, named after their ID. IDs are permanent and never reused.

## References

References are recorded in one direction, from the more volatile artifact to the more stable one, and reverse relationships are derived:

- a contract names the requirements it specifies,
- a requirement names the decisions that shaped it,
- a decision names only the decisions it supersedes.

Decisions stay authoritative unless a newer decision supersedes them. Requirements stay authoritative unless marked `obsolete: true`.

The `why` rule in [`.please/checks/why`](../.please/checks/why/check.py) fails when an artifact violates its schema, an ID is duplicated or a reference does not resolve. Artifacts that contradict each other are resolved explicitly, never silently.
