# Ledger

[![build](https://github.com/please-behave/ledger/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/please-behave/ledger/actions/workflows/ci.yml) [![release](https://img.shields.io/github/v/release/please-behave/ledger?display_name=tag&sort=semver)](https://github.com/please-behave/ledger/releases) [![license](https://img.shields.io/badge/license-PolyForm%20Noncommercial%201.0.0-3949ab.svg)](https://polyformproject.org/licenses/noncommercial/1.0.0/)

[![tests](https://christianpflugradt.github.io/badges-and-stuff/please-behave-ledger/tests.svg)](https://christianpflugradt.github.io/badges-and-stuff/) [![coverage](https://christianpflugradt.github.io/badges-and-stuff/please-behave-ledger/coverage.svg)](https://christianpflugradt.github.io/badges-and-stuff/) [![LOC](https://christianpflugradt.github.io/badges-and-stuff/please-behave-ledger/loc.svg)](https://christianpflugradt.github.io/badges-and-stuff/)

The ledger of the Please Behave platform. It is a service in early development.

> Please Behave is in early development and not ready for production use.

## Running

The ledger stores its data in PostgreSQL and is configured through environment variables:

| Variable | Meaning |
|---|---|
| `LEDGER_DATABASE_URL` | PostgreSQL connection URL. Required. |
| `LEDGER_LISTEN_ADDRESS` | Address to listen on. Defaults to `0.0.0.0:8080`. |

`ledger migrate` applies the embedded migrations and logs each applied migration's version and name. If it applies none, it logs that the schema is already up to date. Starting `ledger` never migrates. `GET /healthz` answers while the process runs, and `GET /readyz` answers only when the database is reachable and has the schema this build requires.

The ledger accepts attestations with `POST /v0/attestations` and serves them with `GET /v0/attestations`, `GET /v0/attestations/latest` and `GET /v0/attestations/{id}`; the contracts are in [why/contracts](why/contracts).

Trusted collectors send repository inventory, default-branch head observations and sync heartbeats through the ledger's [internal platform sync endpoint](why/contracts/ingest/platform-sync.md).

It is released as the container image `ghcr.io/please-behave/ledger`. The purpose and principles of Please Behave are in the [Why repository](https://github.com/please-behave/why).
