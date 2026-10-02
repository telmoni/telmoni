# Development Guide

How to set up, run and check Telmoni locally. How it is built is described in [architecture/](architecture/README.md): the modules and their seams, identity, tenancy, data, background work and deployment.

---

## Environment Variables

| Variable | Description |
|---|---|
| `AUTH_DATABASE_URL`, `NOTIFICATIONS_DATABASE_URL`, `AGENT_DATABASE_URL` | Each module's own PostgreSQL role |
| `MIGRATOR_DATABASE_URL` | The role `migrate` and `rotate` run as |
| `SERVICE_SECRET` | The shared secret the console sends the Rust server; it proves a request came from inside the platform |
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
- **Docker:** Docker engine or Docker Desktop (PostgreSQL 17 with `pgvector`, Redis, Mailpit, and Ollama on request)
- **cargo-deny:** `cargo install cargo-deny`
- **typos:** `cargo install typos-cli` (`make ci` runs it)

### Initial Setup & Running the Stack

Run the dev environment audit:

```console
make doctor
```

Scaffold environment files, database, and migrations:

```console
make first-run
```

Start the full stack: PostgreSQL, Redis, Mailpit, the Rust server and the Next.js console. Set `COMPOSE_PROFILES=ollama` in `.env` to start Ollama too.

```console
make up
```

Services are exposed at:
- **Web Console:** [http://localhost:3000](http://localhost:3000)
- **Rust API Server:** [http://localhost:8082](http://localhost:8082)
- **Mailpit:** [http://localhost:8025](http://localhost:8025)

### Signing In Locally

- **No identity provider is needed.** The built-in password form signs you in.
- **Mail.** `SMTP_URL` routes transactional mail to Mailpit.
- **Test sign-in.** `ALLOW_TEST_SESSION=true` also mounts `POST /test/session`, which is how the end-to-end tests sign in. The server refuses to start with it in a Kubernetes pod.

### Running Verification & CI Gates

Run the comprehensive CI gate mirroring `.github/workflows/ci.yml`:

```console
make ci
```

Run fast local iteration checks:

```console
make check     # typecheck, cargo check, fmt, clippy, rustdoc
make lint      # ESLint across the web console
make test TEST_THREADS=2  # Vitest, then the Rust tests against the local database
make fmt       # Format Rust and TypeScript codebase
```

### Pre-Release Schema Discipline

Before `v0.1.0`, Telmoni avoids migration deprecation shims:
- Each crate owns a single migration file under `crates/<module>/migrations/`.
- Edit the migration file directly for schema updates.
- Rebuild your local database with `make db-reset`.
- Generate updated wire contracts with `make contract`.
