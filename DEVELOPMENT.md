# Telmoni Architecture & Development Guide

This document describes the internal architecture of the Telmoni platform, modular monolith structure, authentication lifecycle, environment configuration, and development workflows.

---

## Architecture Overview

Telmoni is designed as a unified modular monolith: a single Rust server (`telmoni serve`) backed by PostgreSQL with `pgvector`, and a Next.js console operating strictly as a backend-for-frontend (BFF) with Redis for live events and rate limits.

```text
crates/
  telmoni/        The binary entry point (:8082): links modules, wires seams,
                  runs loops, sweeps, and operational CLI tasks (`migrate`, `rotate`, `sweep`).
  auth/           Identity, tenancy, membership, sessions, API tokens, flags, audit reads.
  notifications/  Activity feed, Slack / Discord / webhook delivery engine.
  agent/          Console assistant: hybrid vector + full-text search across docs, audit logs,
                  the feed, deliveries, and past conversations, with citations.
  migrator/       Database migration runner, role hardening, audit schema DDL.
  shared/         Common domain types, errors, RBAC, tenancy context, redaction, and seams (`seam.rs`).
web/              Console UI (Next.js App Router) — backend-for-frontend on :3000.
contract/         Generated wire contracts (`wire-contract.json`, `openapi.json`).
deploy/           Production Helm charts and reference Docker Compose topologies.
scripts/          Dev automation, webhook receivers, and tunneling helpers.
```

### Modular Monolith & Inter-Module Seams

1. **Role-Hardened Database Pools:**
   - Modules do not query each other's tables directly.
   - Each module connects using its own Postgres role (`auth`, `notifications`, `agent`) enforcing row-level security (RLS) within a single process.
2. **Trait Seams (`seam.rs`):**
   - Inter-module communication flows through explicit Rust traits (`crates/shared/src/seam.rs`).
   - `notifications` asks `auth` for identity and tenant flags.
   - `auth` triggers notification dispatches and cascade purge hooks on organization deletion.
   - `agent` indexes audit logs and feed deliveries through authorized reader seams.
3. **Console as a Thin Proxy:**
   - The web console (`web/`) never connects directly to PostgreSQL.
   - It proxies requests to the Rust server (`:8082`) using session bearer tokens and a shared `SERVICE_SECRET`.

---

## Authentication Architecture

Telmoni provides comprehensive multi-tenant authentication without external dependency lock-in:

### 1. Interactive Web & CLI Authentication

- **Web Console:** Built-in account management minting secure, hashed session tokens. Optional OpenID Connect (OIDC) integration via `OIDC_*` variables.
- **CLI Device Authorization (`/cli` Door):** Implements RFC 8628 (OAuth 2.0 Device Authorization Grant) via `POST /cli/auth/device` and `POST /cli/auth/device/poll`.
- **Session Lifecycle:**
  - Active sessions are viewable and revocable from the console settings.
  - Revocations immediately invalidate the session token on subsequent API requests.
- **Local Dev Auth:** Locally, the built-in password form needs no third-party identity provider, and `SMTP_URL` routes transactional email to Mailpit (`http://localhost:8025`). `ALLOW_TEST_SESSION=true` additionally mounts `POST /test/session`, the sign-in door end-to-end tests use.

### 2. API Key Authentication (`/v1`)

- Direct programmatic access for automation, SDKs, and CI/CD pipelines.
- API keys carry the prefix `telmoni_` and authenticate strictly against `/v1` endpoints with tenant scoping.

---

## Environment Variables

| Variable | Description |
|---|---|
| `AUTH_DATABASE_URL`, `NOTIFICATIONS_DATABASE_URL`, `AGENT_DATABASE_URL` | Each module's own PostgreSQL role |
| `MIGRATOR_DATABASE_URL` | The role `migrate` runs as |
| `SERVICE_SECRET` | Shared secret between web console BFF and Rust API server |
| `PORT` | Port for the Rust API server (default: `8082`) |
| `SMTP_URL` | Outgoing mail (Mailpit locally) |
| `AGENT_MODEL_PROVIDER` | Agent model protocol (`anthropic`, or `openai` for any OpenAI-compatible endpoint such as Gemini or Ollama); unset turns the agent off |
| `AGENT_MODEL_URL`, `AGENT_MODEL`, `AGENT_MODEL_API_KEY` | The chat model's endpoint, name and key |
| `EMBEDDINGS_URL`, `EMBEDDINGS_MODEL`, `EMBEDDINGS_API_KEY` | An OpenAI-compatible embeddings endpoint (768 dimensions required) |
| `OIDC_*` | OpenID Connect identity provider configuration (optional) |
| `REDIS_URL` | The console's Redis, in `web/.env.local` |

`.env.example` and `web/.env.local.example` document every variable.

---

## Development Workflows

### Prerequisites

- **Rust:** `1.98.1` (pinned in `rust-toolchain.toml`; auto-installed by `rustup`)
- **Node.js:** Node 24 (pinned in `web/.nvmrc`) and `npm`
- **Docker:** Docker engine or Docker Desktop (PostgreSQL 17 with `pgvector`, Redis, Mailpit, Ollama)
- **cargo-deny:** `cargo install cargo-deny`

### Initial Setup & Running the Stack

Run the dev environment audit:

```console
make doctor
```

Scaffold environment files, database, and migrations:

```console
make first-run
```

Start the full stack (PostgreSQL, Redis, Mailpit, Ollama, Rust server, Next.js console):

```console
make up
```

Services are exposed at:
- **Web Console:** [http://localhost:3000](http://localhost:3000)
- **Rust API Server:** [http://localhost:8082](http://localhost:8082)
- **Mailpit:** [http://localhost:8025](http://localhost:8025)

### Running Verification & CI Gates

Run the comprehensive CI gate mirroring `.github/workflows/ci.yml`:

```console
make ci
```

Run fast local iteration checks:

```console
make check     # Rust clippy, fmt, typecheck
make lint      # ESLint across the web console
make test TEST_THREADS=2  # Vitest + Rust integration tests against local DB
make fmt       # Format Rust and TypeScript codebase
```

### Pre-Release Schema Discipline

Before `v0.1.0`, Telmoni avoids migration deprecation shims:
- Each crate owns a single migration file under `crates/<module>/migrations/`.
- Edit the migration file directly for schema updates.
- Rebuild your local database with `make db-reset`.
- Generate updated wire contracts with `make contract`.
