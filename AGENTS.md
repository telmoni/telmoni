# Telmoni — Agent Guidelines

The open-source Telmoni platform ([`telmoni/telmoni`](https://github.com/telmoni/telmoni)): organizations and projects, members and roles, API keys (`api_tokens` in the code; customer copy always says *API key*), notifications, an audit log and a console agent. One Rust binary, `telmoni`, runs auth, notifications and the console agent as modules; the Next.js console in `web/` sits beside it; the Helm chart in `deploy/` deploys both to GKE. A deployment that needs more builds its own binary and console on top (README § Extending Telmoni); nothing here names one.

## Ground Rules
- **Pre-launch:** nothing has shipped. Fix schemas, wire contracts, Redis keys and APIs directly to their ideal state; no migration paths, dual-writes, shims or versioned schemas.
- **Quality gate:** never weaken a test or leave the tree broken.
- **Ask first:** any dependency change (any edit to `Cargo.toml`, `Cargo.lock`, `web/package.json` or `web/package-lock.json`, lockfile-only refreshes included) and any external-state change (`helm install`/`upgrade`, `kubectl apply`, `gcloud` writes, secrets, DNS, a hosted model or embeddings call with a real key).
- **No new migration files:** edit the module's existing migration, and give a new module exactly one; rebuild with `make db-reset` when asked.

## Where Things Live
| Path | What |
|---|---|
| `crates/telmoni` | The binary: `serve` (every module on `PORT` 8082, the sweeps and the agent's indexer on timers) and one-shot admin subcommands (`cli.rs`). `App::assemble` is what a deployment's own binary calls. |
| `crates/auth` | Sign-in (password form, OIDC), identities, organizations, projects, members, invitations, transfers, API keys, sessions, flags, export and deletion, `/v1`. |
| `crates/notifications` | The in-app feed, Slack/Discord/webhook connectors, the delivery queue. |
| `crates/agent` | The console agent, read-only: model adapters, embeddings, the index, hybrid search, tools. Needs the `vector` extension, which a superuser installs. Off until `AGENT_MODEL_PROVIDER` is set. |
| `crates/shared` | `TelmoniError`, RBAC (`rbac.rs`), tenancy scopes, the seams between modules (`seam.rs`), audit, `Redacted`. `tests/` holds the cross-module suites. |
| `crates/migrator` | The migration runner and the `audit` schema; grants, role hardening and the local roles and extensions in `sql/`. Each module's SQL is in `crates/<module>/migrations/`. |
| `web/` | `app/` routes and their tests, `lib/` logic and its tests, `components/` UI (no tests), `content/docs/` the book (the customer docs, served at `/docs`), `e2e/` Playwright, `proxy.ts` middleware, `lib/extension/` and `components/extension/` the slots a console built on this one fills. |
| `contract/` | Generated wire contract and OpenAPI document (`make contract`); never hand-edited. |
| `scripts/` | `up.sh` and the webhook tunnel and receiver. Ask before adding one. |
| `deploy/` | The Helm chart (`charts/telmoni`) and the self-host compose (`compose/`). |
| `ARCHITECTURE.md` | The whole platform on one page, linking down to `docs/`. |
| `docs/` | How it is built, one page per area; `README.md` there is the index. |
| `scratch/` | The maintainer's notes, gitignored. Never write to it. |
| [`telmoni/telmoni-cli`](https://github.com/telmoni/telmoni-cli) | Sibling repo, checked out as `../telmoni-cli`: the CLI and client SDKs. |

## Commands
- **After every change** (no permission needed; report failures verbatim): `make check` (typecheck, cargo check, fmt, clippy, rustdoc), then `make lint`.
- **Only when asked:** `make test TEST_THREADS=2`, `make test-svc SVC=<crate> TEST_THREADS=2 [TEST_FILTER=<name>]`, `make ci`, `make e2e`, `npm run test` in `web/`, `make db-reset`. Never bare `cargo test`: `make test` starts Postgres, and `make test-svc` expects it running. Unasked, say "untested" and name the command (`make db-reset` first if a migration changed).
- **After a wire-contract change:** `make contract`, then update `web/lib/types/enums.ts` (and `web/lib/slug.ts`, which it also pins) until `contract.test.ts` passes.
- **Locally:** `make up` starts `docker-compose.yml` (Postgres, Redis, Mailpit; Ollama with `COMPOSE_PROFILES=ollama`), then runs the server (`:8082`) and console (`:3000`) in one terminal; `make server-dev` and `make web-dev` run either alone.

## Code Rules
- **Rust owns the logic:** business rules, queries, audit, RBAC and tenancy, session resolution. Log only through `tracing` (no `println!` or `dbg!` outside tests). Return `TelmoniError` as RFC 9457 problem details; every `type` needs a row in the book's `web/content/docs/errors.mdx` (`crates/shared/tests/error_catalog.rs`). Never leak a panic or stack trace.
- **Never log secrets or PII**, at any level, nor put them in an error `detail` or panic: no keys, tokens, signing or service secrets, confirmation codes, `Authorization`/`Cookie`/`x-service-secret` headers, emails, names or free text. Log ids. Hold secrets in `Redacted` and read them with `.expose()`.
- **`web/` is a thin proxy:** no database access or business logic. Call the server only with `fetchWithTimeout`/`tryFetchWithTimeout` (`web/lib/api/fetch.ts`) and headers from `web/lib/server/entities/identity-context.ts` (`organizationHeaders`, `projectHeaders`, `personHeaders`, `sessionHeaders`, `accountHeaders`). Log through `@/lib/logger`, never `console.*`. App Router only; server components by default, `"use client"` only at interactive leaves. Test in `web/lib/` and `web/app/`, never `web/components/`.
- **Twelve-factor:** config and backing services (Postgres, Redis, the model, embeddings and rerank URLs) come only from the environment; both processes are stateless; each binds `PORT`; both drain on `SIGTERM`/`SIGINT`; logs are JSON on stdout; admin tasks are subcommands of the binary, never part of the request loop.

## Contracts
- `docs/` is how the code is built, for whoever changes it; a change that alters what a page says updates that page in the same change. What the product does for a customer is the book: MDX under `web/content/docs`, served at `/docs` by `web/app/docs/[[...slug]]`.
- Runnable specs (golden vectors, constants, tests) live in the code, and the book prints them. When one changes: change the code, grep `web/content/docs` for the old value, check constants no test reads (TTLs, retention days, limits) by hand, and fix every page left wrong in the same change. For example, `integrations/webhooks.mdx` prints the golden vector in `web/lib/webhook-signature.ts`.

## Agent Hygiene
- **This repo only:** change nothing in another repository (`telmoni/telmoni-cli`, any other) unless the user says so for this task; that binds subagents too. Reading is fine.
- Edit `AGENTS.md` (`.github/copilot-instructions.md` points here) only when the user asks outright; otherwise propose a diff. Delete any Next.js-generated `web/AGENTS.md`.
- Comments say *why*, never *what*. No AI signatures anywhere.
- No scripted bulk edits: edit each file deliberately (formatters, generators and an approved lockfile write excepted). One-off scripts, backups and logs go in `/tmp/telmoni/`.
- Never read, print, diff or recreate `.env` or `web/.env.local`: they hold real secrets. Change one key by name, after `cp -p` to `/tmp/telmoni/`.
- Keep the repo slim: fix a real problem where it lives. No guard script, lint gate, CI job or make target for a problem that is not happening.

## Git Rules
- Commit only when asked: never `git add` or `git commit` unprompted.
- Conventional commits (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:`) of 1–5 lines: a subject, then optionally a blank line and up to 3 lines on *why*. No file lists, test output, AI signatures or `Co-authored-by` trailers.
- No branches, worktrees, pushes or pull requests.

## Definition of Done
- [ ] `make check` and `make lint` pass, or the failure is reported verbatim.
- [ ] No secret or PII reaches a log, an error `detail` or a commit.
- [ ] The README still describes the tree, `docs/` and the book say what the change does, and every page left wrong in another repository is named.
- [ ] Nothing that needs asking happened unasked (tests, `make db-reset`, dependencies, external state, other repos, commits), and anything untested is reported with its command.
