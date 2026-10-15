# Telmoni

[![CI](https://github.com/telmoni/telmoni/actions/workflows/ci.yml/badge.svg)](https://github.com/telmoni/telmoni/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

Privacy-first telemetry and monitoring for AI agents in production.

> Know when your agents stall, loop or overspend, without sending us a single prompt.

Every run and the steps inside it, every model and tool call with its tokens, latency and cost, the bounds each agent keeps, and the alert when one breaks them. Prompts and completions stay out unless an organization owner explicitly turns them on for a project.

One Rust binary runs the platform behind a Next.js console, backed by PostgreSQL, ClickHouse and Redis. Open source under Apache-2.0.

## What Telmoni does

- **Telemetry for agents** · Ingest over OpenTelemetry's protocol (`/otlp/v1/traces`) or a crontab `curl` to the ping lane (`/ping/{ping_key}/{slug}`). Records runs, steps, model and tool calls, token usage, latency, cost and failures from any framework, correlated by run, session and end user.
- **Monitoring** · Contracts for autonomous agents: crontab schedules, stall timeouts, step caps and cost caps. Tracks state (`healthy`, `late`, `stalled`, `failed`, `paused`) and routes alerts to an in-app feed, Slack and signed webhooks.
- **Privacy first** · Metadata by default: timings, statuses, counts, names and costs. Prompts and completions are dropped at the door unless an organization owner turns content mode on for a project. User IDs are hashed, reads are audited, and deletions are receipted.
- **For developers and agents** · SDKs for Python, TypeScript, Go and Rust. A Next.js console with real-time traces, histograms and session views. An in-app console agent that answers questions with citations. The `telmoni` CLI and agent skills for Claude Code, Cursor and Codex.
- **Self-hostable & extensible** · Deployable via Docker Compose or Kubernetes (Helm). Built on a modular Rust foundation: assemble your own identity provider, mail transport, purge hooks or custom modules.

## Ecosystem

- **Core platform & SDKs** (this repository): the Rust binary, Next.js console and idiomatic SDKs under `sdk/` (Python, TypeScript, Go, Rust).
- **Documentation**: [`telmoni/docs`](https://github.com/telmoni/docs), built into the console and served at [telmoni.com/docs](https://telmoni.com/docs).
- **CLI**: [`telmoni/telmoni-cli`](https://github.com/telmoni/telmoni-cli), terminal access to projects, runs and monitors.
- **Agent skills**: [`telmoni/skills`](https://github.com/telmoni/skills), teaching Claude Code, Cursor, Codex and any agent reading `SKILL.md` to drive the platform.

## Getting started

You need Rust (the version `rust-toolchain.toml` pins, which `rustup` installs for you), Node (`web/.nvmrc`), Docker and `rsync`, and the docs checked out beside this repository, since the console builds them in.

```console
git clone https://github.com/telmoni/docs.git ../docs   # once: the docs, which `make book` lays into the console
make first-run   # once: env files, dependencies, Postgres and ClickHouse, migrations
make up          # Postgres, ClickHouse, Redis, Mailpit, the server on :8082, the console on :3000
```

Sign in with the password form; mail lands in Mailpit at http://localhost:8025. [Developing](https://telmoni.com/docs/contributing/developing#the-platform) has the rest.

## Layout

```text
crates/
  telmoni/        the binary: `serve`, and the admin commands (`migrate`, `rotate`, `sweep`, …)
  auth/           sign-in, organizations and projects, members, invitations, API keys
  notifications/  the feed, the Slack and webhook connectors, delivery
  agent/          the console agent: search over the docs and a project's own data
  telemetry/      ingest at /otlp/v1/traces and the ping lane at /ping, each project's settings,
                  monitors, ping keys and open runs, the outbox, the spans and their hourly totals
                  in ClickHouse, its ClickHouse file and nightly purge, ClickHouse's configuration
                  and local users
  migrator/       the migration runner, grants, role hardening, the audit schema;
                  runs telemetry's ClickHouse file and purge
  shared/         errors, RBAC, tenancy, the seams between modules
web/              the console, a thin proxy in front of the server, and the docs it serves, laid in from ../docs (`web/content/docs`)
sdk/              the SDKs: python/, typescript/, go/, rust/, and the fixtures every suite replays
contract/         the generated wire contract and OpenAPI document
deploy/           the Helm chart and the self-host compose file
scripts/          `up.sh` behind `make up`, `book.sh` behind `make book`, and the webhook tunnel and receiver for testing a connector
ARCHITECTURE.md   how it is built, and why, on one page
```

## Deploying

Self-hosting is written up at [telmoni.com/docs/self-host](https://telmoni.com/docs/self-host/overview): Docker Compose from `deploy/compose`, Kubernetes from `deploy/charts/telmoni`. Both pull the images `.github/workflows/publish.yml` publishes to `ghcr.io/telmoni`.

## Extending Telmoni

A deployment that needs more builds on these crates instead of forking them: `telmoni::App::assemble` takes its own identity provider, mail transport and purge hook; a `telmoni::Module` mounts beside the built-in ones; a console built on this one replaces the slot files under `web/lib/extension/` and `web/components/extension/`, and imports the core only through `lib/extension/ui.ts` and `lib/extension/server.ts`. [ARCHITECTURE.md](ARCHITECTURE.md#extending-the-core) describes the seams.

## Contributing

How to contribute, the AI policy, the Code of Conduct and the security policy are in the docs: [telmoni.com/docs/contributing](https://telmoni.com/docs/contributing/introduction).

## License

Apache-2.0 ([LICENSE](LICENSE), [NOTICE](NOTICE)). Unless you say otherwise, a contribution you submit is licensed the same way.
