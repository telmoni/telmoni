# Telmoni

[![CI](https://github.com/telmoni/telmoni/actions/workflows/ci.yml/badge.svg)](https://github.com/telmoni/telmoni/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

A foundation for organizations and their projects: members and roles, API tokens, notifications, a hash-chained audit log and a console agent, served by one Rust binary behind a Next.js console.

**Pre-release:** Telmoni is currently pre-release. Until the first release (`v0.1.0`), schema changes edit migrations in place. Customer documentation is maintained in the [`telmoni/docs`](https://github.com/telmoni/docs) repository.

---

## Getting Started

### Prerequisites

- Rust 1.98.1 (`rust-toolchain.toml`)
- Node.js 24 (`web/.nvmrc`) and npm
- Docker / Docker Desktop (PostgreSQL 17 with `pgvector`, Redis, Mailpit, Ollama)

### Initial Bootstrap & Running Locally

```console
make first-run     # once: scaffold env files, dependencies, PostgreSQL, and migrations
make up            # launch PostgreSQL, Redis, Mailpit, the server and console
                   # (and Ollama, with COMPOSE_PROFILES=ollama in .env)
```

`make first-run` creates `.env` (the server) and `web/.env.local` (the console) from their examples and mints a shared `SERVICE_SECRET`.

Services will be running at:
- **Web Console:** [http://localhost:3000](http://localhost:3000)
- **Rust Server API:** [http://localhost:8082](http://localhost:8082)
- **Mailpit Web UI:** [http://localhost:8025](http://localhost:8025)

Sign-in requires no third-party provider locally: `auth` holds accounts and mints session tokens directly. Outgoing emails land in Mailpit. An OpenID Connect provider can be connected via `OIDC_*` variables in `.env`.

---

## Repository Layout

```text
architecture/     How Telmoni is built, and why: one page per area.
crates/
  telmoni/        The binary: links modules, wires seams, serves on :8082,
                  runs loops and operational jobs (`migrate`, `rotate`, `sweep`).
  auth/           Identity, tenancy, membership, API tokens, flags, audit log.
  notifications/  Activity feed, Slack / Discord / webhook connectors, delivery engine.
  agent/          Console AI assistant: hybrid search (pgvector + tsvector) over docs,
                  audit logs, the feed, deliveries, and past conversations.
  migrator/       Migration runner, object grants, role hardening, audit DDL.
  shared/         Foundation: config, errors, RBAC, tenancy, logging, and seams (`seam.rs`).
web/              Console UI (Next.js App Router) — backend-for-frontend on :3000.
contract/         Generated wire contracts (`wire-contract.json`, `openapi.json`).
deploy/           Production Helm charts and reference Docker Compose topologies.
scripts/          Dev automation, webhook receivers, and tunneling helpers.
```

---

## Architecture & Data Flow

Telmoni runs as one process, `telmoni serve`, built from library crates. [`architecture/`](architecture/README.md) describes how it is built and why, one page per area:
- the server and its seams;
- identity;
- tenancy;
- data;
- background work;
- deletion;
- notifications;
- the agent;
- the console;
- deployment.

In brief:

- **Separate modules in one process:** Modules never read each other's tables. They ask through traits in `crates/shared/src/seam.rs`.
- **A database role per module:** Each module opens its own pool as its own role (`auth`, `notifications`, `agent`), so row-level security and grants hold inside one process. The self-host compose quickstart connects every module as one superuser, which bypasses both (see [architecture/tenancy.md](architecture/tenancy.md#when-everything-connects-as-a-superuser)).
- **Console as a thin proxy:** The console holds the session and relays requests. Person lanes carry the bearer and the shared `SERVICE_SECRET`. Lanes that run before sign-in carry the secret alone. Every rule lives in Rust.
- **Single migration per module:** Before `v0.1.0`, schema changes edit the single migration file under each crate's `migrations/` directory.

---

## Deploying

The server image (`crates/telmoni/Dockerfile`) serves all roles:
- Default command: `serve`
- Operational tasks: `migrate`, `rotate`, `sweep <name>`, `terminate <org_id>`, `restore <org_id>`

Deployments are supported via:
- **Kubernetes / Helm:** Helm chart located under `deploy/charts/telmoni`.
- **Docker Compose:** Reference multi-container setups under `deploy/compose`.

---

## Extending Telmoni

Deployments requiring custom integrations can compile a custom binary using the same crates:
- `telmoni::App::assemble` accepts custom identity providers, mail transports, and `telmoni_shared::seam::PurgeHook` implementations.
- Modules implementing `telmoni::Module` can be mounted directly onto the server listener.
- Console UI extensions overlay `web/` by replacing the slot files `lib/extension/nav.ts`, `lib/extension/site-nav.ts`, `lib/extension/public-paths.ts` and `components/extension/banner.tsx`. Overlays import only from `lib/extension/ui.ts` and `lib/extension/server.ts`.

---

## Development & Testing

Run the full workspace CI verification gate:

```console
make ci
```

Run component checks during development:

```console
make check                 # Fast checks: typecheck, cargo check, fmt, clippy, rustdoc
make test TEST_THREADS=2   # Integration tests against local PostgreSQL
make fmt                   # Format Rust and TypeScript codebase
```

For setting up and working on the stack, see [DEVELOPMENT.md](DEVELOPMENT.md); for how it is built, see [architecture/](architecture/README.md).

---

## Security

Please report vulnerabilities following our [Security Policy](SECURITY.md). Do not report vulnerabilities through public GitHub issues.

---

## Community & License

- [Contributing](CONTRIBUTING.md) — DCO requirements, code standards, and PR workflows.
- [Code of Conduct](CODE_OF_CONDUCT.md) — Contributor Covenant v2.1.
- [Architecture](architecture/README.md) — How Telmoni is built, and why.
- [Development Guide](DEVELOPMENT.md) — Setting up, running and checking the stack.
- [Security Policy](SECURITY.md) — Vulnerability reporting channels and safe harbor.
- [License](LICENSE) — Licensed under the Apache License, Version 2.0.
