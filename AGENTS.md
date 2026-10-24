# Telmoni — Agent Guidelines

The open-source Telmoni platform ([`telmoni/telmoni`](https://github.com/telmoni/telmoni)): organizations and projects, members and roles, API keys (`api_tokens` in the code; customer copy always says *API key*), notifications, an audit log and a console agent. One Rust binary, `telmoni`, runs auth, notifications, telemetry and the console agent as modules; the Next.js console in `web/` sits beside it; the Helm chart in `deploy/` deploys both to GKE. A deployment that needs more builds its own binary and console on top (README § Extending Telmoni); nothing here names one.

## Ground Rules
- **Pre-launch:** nothing has shipped. Fix schemas, wire contracts, Redis keys and APIs directly to their ideal state; no migration paths, dual-writes, shims or versioned schemas.
- **Quality gate:** never weaken a test or leave the tree broken.
- **Ask first:** any dependency change (any edit to `Cargo.toml`, `Cargo.lock`, `web/package.json`, `web/package-lock.json` or an SDK's manifest or lockfile under `sdk/`, lockfile-only refreshes included) and any external-state change (`helm install`/`upgrade`, `kubectl apply`, `gcloud` writes, secrets, DNS, a hosted model or embeddings call with a real key).
- **No new migration files:** edit the module's existing migration, and give a new module exactly one per store it keeps; rebuild with `make db-reset` when asked.

## Where Things Live
| Path | What |
|---|---|
| `crates/telmoni` | The binary: `serve` (every module on `PORT` 8082, the sweeps and the agent's indexer on timers) and one-shot admin subcommands (`cli.rs`). `App::assemble` is what a deployment's own binary calls. |
| `crates/auth` | Sign-in (password form, OIDC), identities, organizations, projects, members, invitations, transfers, API keys, sessions, flags, export and deletion, `/v1`. |
| `crates/notifications` | The in-app feed, the Slack and webhook connectors, the delivery queue. |
| `crates/agent` | The console agent, read-only: model adapters, embeddings, the index, hybrid search, tools. Needs the `vector` extension, which a superuser installs. Off until `AGENT_MODEL_PROVIDER` is set. |
| `crates/telemetry` | Each project's settings and content mode in Postgres, the mode read and set through the console's lane under `/internal/v1/telemetry` (`handler/`); the spans and their hourly totals in ClickHouse, which the module reads and writes only through `store.rs`, each read naming its projects. `clickhouse/` holds ClickHouse's one migration file, which `migrate` applies (`schema.rs`), its settings and its local users; `rotate` drops the expired days and months (`retention.rs`), both as the migrator's user. |
| `crates/shared` | `TelmoniError`, RBAC (`rbac.rs`), tenancy scopes, the seams between modules (`seam.rs`), audit, `Redacted`. `tests/` holds the cross-module suites. |
| `crates/migrator` | The migration runner and the `audit` schema; grants, role hardening and the local roles and extensions in `sql/`. Each module's SQL is in `crates/<module>/migrations/`, and telemetry's ClickHouse file in `crates/telemetry/clickhouse/`. |
| `web/` | `app/` routes and their tests, `lib/` logic and its tests, `components/` UI (no tests), `content/docs/` the book (the customer docs, served at `/docs`), laid in from `../docs` by `make book` and ignored by git, `e2e/` Playwright, `proxy.ts` middleware, `lib/extension/` and `components/extension/` the slots a console built on this one fills. |
| `sdk/` | The four SDKs (`python/`, `typescript/`, `go/`, `rust/`): an agent's runs, steps, model calls and tool calls as OpenTelemetry spans, sent to `/otlp/v1/traces` by an exporter of each one's own and nowhere else; `fixtures/`, the golden export and the answers every suite replays. `crates/telemetry/src/contract.rs` names what ingest reads, and each SDK's `contract` module copies it. |
| `contract/` | Generated wire contract and OpenAPI document (`make contract`); never hand-edited. |
| `scripts/` | `up.sh`, `book.sh` (the book, laid into the console) and the webhook tunnel and receiver. Ask before adding one. |
| `deploy/` | The Helm chart (`charts/telmoni`) and the self-host compose (`compose/`). |
| `ARCHITECTURE.md` | How the platform is built, and why, on one page: the repository's one internal document. |
| `scratch/` | The maintainer's notes, gitignored. Never write to it. |
| [`telmoni/docs`](https://github.com/telmoni/docs) | Sibling repo, checked out as `../docs`: the book, `content/`, which `make book` lays into `web/content/docs`, and `hosted/`, the hosted service's own pages, which this console never serves. |
| [`telmoni/telmoni-cli`](https://github.com/telmoni/telmoni-cli) | Sibling repo, checked out as `../telmoni-cli`: the CLI, a client of the `/cli` and `/v1` lanes and nothing else. |

## Commands
- **After every change** (no permission needed; report failures verbatim): `make check` (typecheck, cargo check, fmt, clippy, rustdoc), then `make lint`.
- **After a change under `sdk/`:** `make sdk-check` too (the TypeScript, Go and Python suites, and `cargo test -p telmoni`, the Rust SDK's, which needs no database); the Rust SDK is a workspace member, so `make check` already covers it.
- **After a change in `../docs`:** `make book-check` too, the book's own gate (its `make check` runs the same): the console built with the pages, and the tests that read them.
- **Only when asked:** `make test TEST_THREADS=2`, `make test-svc SVC=<crate> TEST_THREADS=2 [TEST_FILTER=<name>]`, `make ci`, `make e2e`, `npm run test` in `web/`, `make db-reset`. Never bare `cargo test`: `make test` starts Postgres and ClickHouse, and `make test-svc` expects both running. Unasked, say "untested" and name the command (`make db-reset` first if a migration changed).
- **After a wire-contract change:** `make contract`, then update `web/lib/types/enums.ts` (and `web/lib/slug.ts`, which it also pins) until `contract.test.ts` passes.
- **Locally:** `make up` starts `docker-compose.yml` (Postgres, ClickHouse, Redis, Mailpit; Ollama with `COMPOSE_PROFILES=ollama`, Dex with `COMPOSE_PROFILES=oidc`), then runs the server (`:8082`) and console (`:3000`) in one terminal, with the book laid in from `../docs` and each edit there laid in within a second; `make server-dev` and `make web-dev` run either alone.

## Code Rules
- **Rust owns the logic:** business rules, queries, audit, RBAC and tenancy, session resolution. Log only through `tracing` (no `println!` or `dbg!` outside tests). Return `TelmoniError` as RFC 9457 problem details; every `type` needs a row in the book's `errors.mdx`, `../docs/content/errors.mdx` (`crates/shared/tests/error_catalog.rs`). Never leak a panic or stack trace.
- **Never log secrets or PII**, at any level, nor put them in an error `detail` or panic: no keys, tokens, signing or service secrets, confirmation codes, `Authorization`/`Cookie`/`x-service-secret` headers, emails, names or free text. Log ids. Hold secrets in `Redacted` and read them with `.expose()`.
- **`web/` is a thin proxy:** no database access or business logic. Call the server only with `fetchWithTimeout`/`tryFetchWithTimeout` (`web/lib/api/fetch.ts`) and headers from `web/lib/server/entities/identity-context.ts` (`organizationHeaders`, `projectHeaders`, `personHeaders`, `sessionHeaders`, `accountHeaders`). Log through `@/lib/logger`, never `console.*`. App Router only; server components by default, `"use client"` only at interactive leaves. Test in `web/lib/` and `web/app/`, never `web/components/`.
- **Every route names its version after its lane, `v1` today:** the console's lanes on the server under `/internal/v1`, the CLI's under `/cli/v1`, a vendor's webhook at `/webhooks/v1/<vendor>`, the Route Handlers the console's pages fetch under `web/app/api/v1/`; the read API is `/v1`, and ingest OTLP's own `/otlp/v1/traces`. The probes (`/health`, `/livez`, `/api/health`), the console's pages and the handlers a browser is sent to rather than fetches (`/auth/*`, `/connect/*`, the trace list's download, `llms.txt` and `llms-full.txt`) carry none. Before launch a lane's `v1` changes in place, as every contract does; after it, a breaking change is a `v2` served beside the `v1` until its clients move.
- **The SDKs grow together:** a behaviour added to one lands in all four in the same change, with its row in `ARCHITECTURE.md`'s parity table (*The SDKs*) and, where the wire or an answer changes, a fixture in `sdk/fixtures/`. No SDK reads an `OTEL_*` variable, prints or serializes its key, sends a span outside a run, follows a redirect, or calls anything but ingest.
- **Twelve-factor:** config and backing services (Postgres, ClickHouse, Redis, the model, embeddings and rerank URLs) come only from the environment; both processes are stateless; each binds `PORT`; both drain on `SIGTERM`/`SIGINT`; logs are JSON on stdout; admin tasks are subcommands of the binary, never part of the request loop.

## Contracts
- `ARCHITECTURE.md` is how the code is built, for whoever changes it; a change that alters what it says updates it in the same change. What the product does for a customer is the book: MDX in `telmoni/docs`, `../docs/content`, which `make book` lays into `web/content/docs` and `web/app/docs/[[...slug]]` serves at `/docs`.
- Runnable specs (golden vectors, constants, tests) live in the code, and the book prints them. When one changes: change the code, grep `../docs/content` for the old value, check constants no test reads (TTLs, retention days, limits) by hand, and fix every page left wrong in the same task. For example, `integrations/webhooks.mdx` prints the golden vector in `web/lib/webhook-signature.ts`.

## Agent Hygiene
- **This repo only:** change nothing in another repository (`telmoni/telmoni-cli`, any other) unless the user says so for this task; that binds subagents too. Reading is fine. The book is the exception: a change here that leaves a page in `../docs` wrong fixes that page there, in the same task.
- Edit `AGENTS.md` (`.github/copilot-instructions.md` points here) only when the user asks outright; otherwise propose a diff. Delete any Next.js-generated `web/AGENTS.md`.
- Comments are for a person reading the code, and short: they say *why*, never *what* — a doc comment in a line or two, an inline comment in one, a test's doc in one line on what it proves. The reasoning behind a change (a race worked through, the designs turned down, its history) goes in `ARCHITECTURE.md` or the commit message, never above a function, and editing a comment means shortening its whole block. No AI signatures anywhere.
- No scripted bulk edits: edit each file deliberately (formatters, generators and an approved lockfile write excepted). One-off scripts, backups and logs go in `/tmp/telmoni/`.
- Never read, print, diff or recreate `.env` or `web/.env.local`: they hold real secrets. Change one key by name, after `cp -p` to `/tmp/telmoni/`.
- Keep the repo slim: fix a real problem where it lives. No guard script, lint gate, CI job or make target for a problem that is not happening.

## Git Rules
- Commit only when asked: never `git add` or `git commit` unprompted.
- Conventional commits (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:`) of 1–5 lines and the sign-off: a subject, then optionally a blank line and up to 3 lines on *why*. No file lists, test output, AI signatures or `Co-authored-by` trailers.
- Every commit is signed off by the developer it is made for, certifying the [Developer Certificate of Origin](https://developercertificate.org/): `git commit -s` adds `Signed-off-by: Name <email>` from git's `user.name` and `user.email`, which name a person, never an agent.
- No branches, worktrees, pushes or pull requests.

## Definition of Done
- [ ] `make check` and `make lint` pass, or the failure is reported verbatim.
- [ ] No secret or PII reaches a log, an error `detail` or a commit.
- [ ] The README still describes the tree, `ARCHITECTURE.md` and the book (`../docs`) say what the change does, and every page left wrong in another repository is named.
- [ ] Nothing that needs asking happened unasked (tests, `make db-reset`, dependencies, external state, other repos, commits), and anything untested is reported with its command.
