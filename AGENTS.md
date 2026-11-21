# Working on the Ledger

## Why

- `why/` is the source of truth for the ledger's behavior, responsibilities and decisions. Read `why/README.md` and the artifacts relevant to the task before implementation. Purpose, principles and direction of Please Behave are in the central Why repository (https://github.com/please-behave/why).
- Write requirements and decisions together with the code that implements them, and record only what holds in the medium term.
- The `why` rule validates requirements, decisions and API contract indices against the schemas in `why/` and resolves the references between them. Update `why/README.md` when an artifact kind is added.
- Define the contract and acceptance criteria before changing behavior. If artifacts conflict or code differs from them, investigate before proceeding. Never silently redefine behavior.

## Database migrations

- Migrations are plain SQL files and only go forward. Never edit one that has been applied anywhere.
- Every schema change takes at least two steps. First expand: additive and compatible with the release before. The code that uses it follows in a later release. Contract only when no running version needs the old shape.
- A migration never holds long exclusive locks. Create indexes concurrently, outside a transaction, and backfill in batches.
- Starting an instance never migrates. The `migrate` subcommand applies migrations, and readiness reports whether the required schema is present.

## Data

- Attestations are appended and never updated or deleted through the API.
- The database is reached only through bound parameters.

## Tests

- Integration tests use a throwaway PostgreSQL and never a shared one.
