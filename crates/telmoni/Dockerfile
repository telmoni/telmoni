# syntax=docker/dockerfile:1.7
# The server image. Build from the repo root:
#   docker build -f crates/telmoni/Dockerfile -t telmoni .
#
# One binary, every job: the image's default command is `serve`; a Job
# overrides it with `migrate`, `rotate`, `sweep <name>` or `terminate <org>`
# as the container's `args`.
#
# Required env at runtime, `serve` and the sweeps:
#   AUTH_DATABASE_URL, NOTIFICATIONS_DATABASE_URL, TELEMETRY_DATABASE_URL,
#   TELEMETRY_CLICKHOUSE_URL, SERVICE_SECRET, APP_URL
# Required, `migrate` and `rotate`:
#   MIGRATOR_DATABASE_URL (the migrator role; every set under /app/migrations)
#   and MIGRATOR_CLICKHOUSE_URL (its ClickHouse user; telemetry's ClickHouse
#   file is compiled into the binary)
# Optional:
#   PORT (default 8082), LOG_LEVEL (default info), RUN_SWEEPS (default true),
#   the OIDC_* provider, SMTP_URL, MAIL_FROM, the SLACK_* and DISCORD_* apps,
#   CONNECTOR_KEK, DATABASE_CA_CERT (required in a pod), and the agent:
#   AGENT_DATABASE_URL, AGENT_MODEL_* and EMBEDDINGS_* (crates/agent/src/config.rs)

# The compiler rust-toolchain.toml pins. This build never reads that file, so
# the two move together.
FROM rust:1.98.1-slim-bookworm@sha256:ff521445a372125ed4f76e1453a1f8098f2d05332d1601d30db1c1f62757e730 AS chef
RUN cargo install cargo-chef --version 0.1.78 --locked \
 && rustup target add x86_64-unknown-linux-musl \
 && apt-get update \
 && apt-get install -y --no-install-recommends musl-tools \
 && rm -rf /var/lib/apt/lists/*

FROM chef AS planner
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --target x86_64-unknown-linux-musl --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --locked --target x86_64-unknown-linux-musl -p telmoni

FROM gcr.io/distroless/static-debian13:nonroot@sha256:e2e927ec666bae08560abb3c55d0659eceabb657f56b6782ab500a9fc7f555e3
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/telmoni /app/telmoni
# `telmoni migrate` reads /app/migrations/<schema>, every directory there,
# `audit` first — every module's `emit_audit` needs `audit.events` to exist.
# An image built FROM this one adds a set by copying it in beside these, and
# its grants as one file under /app/grants.
COPY --from=builder /app/crates/migrator/migrations      /app/migrations/audit
COPY --from=builder /app/crates/auth/migrations          /app/migrations/auth
COPY --from=builder /app/crates/notifications/migrations /app/migrations/notifications
COPY --from=builder /app/crates/agent/migrations         /app/migrations/agent
COPY --from=builder /app/crates/telemetry/migrations     /app/migrations/telemetry
EXPOSE 8082
ENTRYPOINT ["/app/telmoni"]
CMD ["serve"]
