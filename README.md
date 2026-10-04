# Telmoni

[![CI](https://github.com/telmoni/telmoni/actions/workflows/ci.yml/badge.svg)](https://github.com/telmoni/telmoni/actions/workflows/ci.yml)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)

A foundation for organizations and their projects: members and roles, API keys, notifications, a hash-chained audit log and a console agent. One Rust binary runs all of it behind a Next.js console.

Telmoni has not launched yet. The customer documentation is at [docs.telmoni.com](https://docs.telmoni.com) ([`telmoni/docs`](https://github.com/telmoni/docs)); the CLI and SDKs are in [`telmoni/telmoni-cli`](https://github.com/telmoni/telmoni-cli).

## Getting started

You need Rust (the version `rust-toolchain.toml` pins, which `rustup` installs for you), Node (`web/.nvmrc`) and Docker.

```console
make first-run   # once: env files, dependencies, Postgres, migrations
make up          # Postgres, Redis, Mailpit, the server on :8082, the console on :3000
```

Sign in with the password form; mail lands in Mailpit at http://localhost:8025. [Developing](https://docs.telmoni.com/contributing/developing/#the-platform) has the rest.

## Layout

```text
crates/
  telmoni/        the binary: `serve`, and the admin commands (`migrate`, `rotate`, `sweep`, …)
  auth/           sign-in, organizations and projects, members, invitations, API keys
  notifications/  the feed, the Slack, Discord and webhook connectors, delivery
  agent/          the console agent: search over the docs and a project's own data
  migrator/       the migration runner, grants, role hardening, the audit schema
  shared/         errors, RBAC, tenancy, the seams between modules
web/              the console: a thin proxy in front of the server
contract/         the generated wire contract and OpenAPI document
deploy/           the Helm chart and the self-host compose file
architecture/     how it is built, and why, one page per area
```

## Deploying

Self-hosting is written up at [docs.telmoni.com/self-host](https://docs.telmoni.com/self-host/overview/): Docker Compose from `deploy/compose`, Kubernetes from `deploy/charts/telmoni`. Both pull the images `.github/workflows/publish.yml` publishes to `ghcr.io/telmoni`.

## Extending Telmoni

A deployment that needs more builds on these crates instead of forking them: `telmoni::App::assemble` takes its own identity provider, mail transport and purge hook; a `telmoni::Module` mounts beside the built-in ones; a console built on this one replaces the slot files under `web/lib/extension/` and `web/components/extension/`, and imports the core only through `lib/extension/ui.ts` and `lib/extension/server.ts`. [architecture/](architecture/README.md) describes the seams.

## Contributing

How to contribute, the AI policy, the Code of Conduct and the security policy are on the docs site: [docs.telmoni.com/contributing](https://docs.telmoni.com/contributing/introduction/).

## License

Apache-2.0 ([LICENSE](LICENSE), [NOTICE](NOTICE)). Unless you say otherwise, a contribution you submit is licensed the same way.
