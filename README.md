<div align="center">
  <a href="https://telmoni.com">
    <picture>
      <source media="(prefers-color-scheme: dark)" srcset=".github/assets/lockup-color-on-dark.svg">
      <img src=".github/assets/lockup-color-on-light.svg" alt="telmoni" width="360">
    </picture>
  </a>
  <h3>Privacy-first telemetry and monitoring for AI agents</h3>
  <p>
    <a href="https://telmoni.com/docs"><strong>Docs</strong></a> ·
    <a href="#quickstart"><strong>Quickstart</strong></a> ·
    <a href="https://telmoni.com/docs/self-host/overview"><strong>Self-host</strong></a> ·
    <a href="ARCHITECTURE.md"><strong>Architecture</strong></a> ·
    <a href="https://telmoni.com/docs/contributing/introduction"><strong>Contribute</strong></a>
  </p>
  <p>
    <a href="https://github.com/telmoni/telmoni/actions/workflows/ci.yml"><img src="https://github.com/telmoni/telmoni/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
    <a href="LICENSE"><img src="https://img.shields.io/badge/license-Apache--2.0-787880" alt="License: Apache-2.0"></a>
    <a href="ARCHITECTURE.md#where-it-goes-next"><img src="https://img.shields.io/badge/status-pre--release-787880" alt="Status: pre-release"></a>
  </p>
</div>

> Know when your agents stall, loop or overspend, without sending us a single prompt.

Telmoni is an open-source telemetry platform for AI agents. Its SDKs record each **run** of an agent, and the **steps**, **model calls** and **tool calls** inside it, as OpenTelemetry spans; the console reads them back as traces, sessions and users, and alerts on them. Only metadata is kept: prompts and completions come off a span before it leaves your process, and ingest drops any that arrive anyway. It is one Rust binary and a Next.js console on PostgreSQL, ClickHouse and Redis, self-hosted with Docker Compose or Helm.

> [!NOTE]
> Telmoni hasn't launched yet, and it's built in the open. Running today: organizations and roles, API keys, the audit log, notifications, the console agent, and the first of the telemetry on them, the SDKs, ingest, the console's traces, sessions and users, its alerts, a cost cap among them, the cost a call reports, and the SDKs' guard, which stops a run at the caps on its steps and spend you set. Next: an organization's open incidents on its Overview, and content an owner chooses to keep. [Where it goes next](ARCHITECTURE.md#where-it-goes-next) lists what's settled but not built.

## Features

- **[Agent telemetry](https://telmoni.com/docs/api/sdks)**: open a run around your agent, and each step, model call and tool call inside it becomes a span with its timings, status, model, tokens and errors, tied to its run, session and end user.
- **[Private by default](https://telmoni.com/docs/api/field-inventory)**: timings, statuses, counts and names, never content. End-user IDs are hashed with a key unique to each project, and only an organization owner can change a project's content mode.
- **[Traces, sessions and users](https://telmoni.com/docs/workspace/traces)**: filter, sort and chart runs, save views for the project, download CSV or JSON, open a trace as the tree its spans ran in or as a timeline, and see new runs the moment they arrive.
- **[Alerts](https://telmoni.com/docs/workspace/alerts)**: failures, latency, tokens or what a run spends, over an hour, a day or a week, against an alert threshold and an optional warning one; each incident told to the feed and to the Slack channels and webhooks an alert links, in the hours each is set to, with maintenance windows.
- **[Organizations and audit](https://telmoni.com/docs/workspace/audit-log)**: owner, admin and member roles, invitations, transfers and scoped API keys; sign-in by password or any OIDC provider; and an audit log hash-chained per organization, exportable.
- **[Notifications](https://telmoni.com/docs/integrations/notifications)**: an in-app feed, Slack, and webhooks signed with HMAC-SHA256, through a delivery queue that retries.
- **[Console agent](https://telmoni.com/docs/self-host/agent)**: questions in plain language, answered from the docs, the audit log and the feed with sources cited, on Anthropic or any OpenAI-compatible server such as Ollama, vLLM or OpenRouter.

## Quickstart

### 1. Run Telmoni

[Self-host it](#self-hosting) with Docker Compose or Helm, or run it from source with `make up` ([Developing](#developing)): the console on http://localhost:3000, the server on http://localhost:8082.

### 2. Create a project and an API key

Sign in, create a project, and on its **API keys** page create a key with the **ingest** scope.

### 3. Record a run

Each SDK is `telmoni` on its registry: `pip install telmoni`, `npm install telmoni`, `cargo add telmoni`, or `go get github.com/telmoni/telmoni/sdk/go`.

```console
pip install telmoni openai
export TELMONI_ENDPOINT=http://localhost:8082   # your server; plain HTTP is refused unless it's this machine
export TELMONI_API_KEY=...                      # the ingest key from step 2
```

```python
from openai import OpenAI
from telmoni import Telmoni, Usage

telmoni = Telmoni.from_env()
openai = OpenAI()


@telmoni.run("support-bot")
def answer(question: str) -> str:
    with telmoni.model_call("gpt-4o-mini", provider="openai") as call:
        reply = openai.chat.completions.create(
            model="gpt-4o-mini",
            messages=[{"role": "user", "content": question}],
        )
        # The model and its tokens are recorded; the prompt and the reply stay here.
        call.answered(
            response_model=reply.model,
            usage=Usage(
                input_tokens=reply.usage.prompt_tokens,
                output_tokens=reply.usage.completion_tokens,
            ),
        )
    return reply.choices[0].message.content


print(answer("Where is my order?"))
telmoni.flush()
```

TypeScript, Go and Rust work the same way; the [SDK docs](https://telmoni.com/docs/api/sdks) show each.

### 4. See it in the console

The project's **Traces** page shows the run with its model call, tokens and timings. **Sessions** and **Users** group runs by the `session_id` and `user_id` a run is given.

## Self-hosting

Both paths pull the images [`publish.yml`](.github/workflows/publish.yml) publishes to the GitHub Container Registry, `ghcr.io/telmoni/server` and `ghcr.io/telmoni/web` (linux/amd64):

- **[Docker Compose](https://telmoni.com/docs/self-host/docker-compose)**: `deploy/compose/docker-compose.yml` runs the server, the console, PostgreSQL, ClickHouse and Redis on one machine. Write its `.env` once, then `docker compose -f deploy/compose/docker-compose.yml up -d`.
- **[Kubernetes](https://telmoni.com/docs/self-host/kubernetes)**: the chart in `deploy/charts/telmoni` deploys the server, the console and the migration job.

Configuration comes only from the environment ([every setting](https://telmoni.com/docs/self-host/configuration)); [Production](https://telmoni.com/docs/self-host/production) covers OIDC, database role hardening, the reverse proxy and backups. A self-hosted Telmoni phones nothing home: the server calls only the services you configure.

## Integrations

| Integration | For | What it does |
|---|---|---|
| [SDKs](https://telmoni.com/docs/api/sdks) | Python, TypeScript, Go, Rust | Record an agent's runs, steps, model calls and tool calls as spans. |
| [Coding-agent plugins](https://telmoni.com/docs/api/coding-agents) | Claude Code, Codex, OpenCode, Cursor | Record each turn of a coding agent you already use as a run, with no code to write and never a prompt or a file. |
| [CLI](https://github.com/telmoni/telmoni-cli) | macOS, Linux | `telmoni`: device-code sign-in and your organizations from a terminal, or an API key in CI. |
| [Agent skills](https://github.com/telmoni/skills) | Claude Code, Codex, Cursor | Teach a coding agent the SDKs, the plugins, the CLI, the API, webhooks and self-hosting. |
| [Slack](https://telmoni.com/docs/integrations/slack) | Notifications | A project's notifications, posted to a channel. |
| [Webhooks](https://telmoni.com/docs/integrations/webhooks) | Notifications | A project's notifications, delivered to your endpoint and signed with HMAC-SHA256. |
| [OIDC](https://telmoni.com/docs/self-host/configuration) | Any OIDC provider | Single sign-on beside the password form, or instead of it. |
| [Read API](https://telmoni.com/docs/api/reference) | HTTP, JSON | `/v1`: the organization and its members, read with an API key. |

## How it works

<picture>
  <source media="(prefers-color-scheme: dark)" srcset=".github/assets/how-it-works-dark.svg">
  <img src=".github/assets/how-it-works-light.svg" alt="The SDKs in your agents send spans to /otlp/v1/traces, and the CLI and scripts call /cli/v1 and /v1, on one Rust binary that runs auth, telemetry, notifications and the console agent; people use the Next.js console, which calls it; it keeps its data in PostgreSQL, ClickHouse and Redis, and sends notifications to Slack and webhooks.">
</picture>

One Rust binary, `telmoni-server`, runs every module, auth, notifications, telemetry and the console agent, in one process. Each module keeps its own Postgres schema under its own role, behind forced row-level security, and the spans and their hourly totals live in ClickHouse. The console is a thin proxy, so every rule lives in Rust. [ARCHITECTURE.md](ARCHITECTURE.md) explains how it's built, and why.

## Extending Telmoni

A deployment that needs more builds on these crates instead of forking them: `telmoni_server::App::assemble` takes its own identity provider, mail transport and purge hook, a `telmoni_server::Module` mounts beside the built-in ones, and a console built on this one replaces the slot files under `web/lib/extension/` and `web/components/extension/`. [ARCHITECTURE.md](ARCHITECTURE.md#extending-the-core) describes the seams.

## Developing

You need Rust (the version `rust-toolchain.toml` pins, which `rustup` installs for you), Node with npm (`web/.nvmrc`) and Docker.

```console
make first-run   # once: env files, dependencies, Postgres and ClickHouse, migrations
make up          # Postgres, ClickHouse, Redis, Mailpit, the server on :8082, the console on :3000
```

Sign in with the password form; mail lands in Mailpit at http://localhost:8025. [Developing](https://telmoni.com/docs/contributing/developing#the-platform) has the rest, and [`AGENTS.md`](AGENTS.md) holds the rules the code is held to.

<details>
<summary><strong>Repository layout</strong></summary>

```text
crates/
  telmoni/        the binary: `serve`, and the admin commands (`migrate`, `rotate`, `sweep`, …)
  auth/           sign-in, organizations and projects, members, invitations, API keys
  notifications/  the feed, the Slack and webhook connectors, delivery
  agent/          the console agent: models, embeddings, the index and its hybrid search, read-only tools
  telemetry/      ingest at /otlp/v1/traces, each project's settings, the spans and their hourly
                  totals in ClickHouse, its ClickHouse file and nightly purge, and the model
                  prices, converted from LiteLLM's by `make prices`
  migrator/       the migration runner, grants, role hardening, the audit schema
  shared/         errors, RBAC, tenancy, the seams between modules
web/              the console, a thin proxy in front of the server, and the docs it serves (content/docs)
sdk/              the SDKs: python/, typescript/, go/, rust/, and the fixtures every suite replays
contract/         the generated wire contract and OpenAPI document
deploy/           the Helm chart and the self-host compose file
scripts/          `up.sh` behind `make up`, and the webhook tunnel and receiver
.github/          CI, the image publishing workflow, and the logo and diagram this README shows
ARCHITECTURE.md   how it is built, and why, on one page
```

</details>

## Support and contributing

[The docs](https://telmoni.com/docs) cover self-hosting, the console, the SDKs, the CLI, the API and every [error](https://telmoni.com/docs/errors). For a bug or a feature, [open an issue](https://github.com/telmoni/telmoni/issues/new/choose) from a template; for anything else, [SUPPORT.md](https://github.com/telmoni/.github/blob/main/SUPPORT.md) says which channel handles what. Contributions are welcome: start with the [introduction](https://telmoni.com/docs/contributing/introduction), and read the [AI policy](https://telmoni.com/docs/contributing/ai-policy) and the [Code of Conduct](https://telmoni.com/docs/contributing/code-of-conduct) before opening a pull request.

## Security

Please don't report a vulnerability in a public issue. Report it privately from this repository's [Security tab](https://github.com/telmoni/telmoni/security) or to hello@telmoni.com, as the [security policy](https://telmoni.com/docs/legal/security#reporting-a-vulnerability) explains.

## License

Apache-2.0 ([LICENSE](LICENSE), [NOTICE](NOTICE)). Unless you say otherwise, a contribution you submit is licensed the same way.
