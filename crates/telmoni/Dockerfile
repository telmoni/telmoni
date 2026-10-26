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
#   TELEMETRY_CLICKHOUSE_URL, TELEMETRY_KEK, REDIS_URL, SERVICE_SECRET, APP_URL
# Required, `migrate` and `rotate`:
#   MIGRATOR_DATABASE_URL (the migrator role; every set under /app/migrations)
#   and MIGRATOR_CLICKHOUSE_URL (its ClickHouse user; telemetry's ClickHouse
#   file is compiled into the binary)
# Optional:
#   PORT (default 8082), LOG_LEVEL (default info), RUN_SWEEPS (default true),
#   TRUSTED_PROXY_HOPS (default 2), TELMONI_API_KEY and TELMONI_ENDPOINT,
#   the AUTH_CUSTOM_* provider, SMTP_URL, MAIL_FROM, the SLACK_* app,
#   CONNECTOR_KEK, DATABASE_CA_CERT (required in a pod), and the agent:
#   AGENT_DATABASE_URL, AGENT_MODEL_* and EMBEDDINGS_* (crates/agent/src/config.rs)

# The compiler rust-toolchain.toml pins. This build never reads that file, so
# the two move together.
FROM rust:1.99.1-slim-bookworm@sha256:a24e292b4af7bdee351a31d3a6379972a5652f0ea22a1b6d6883324ac4f5097c AS chef
RUN cargo install cargo-chef --version 0.1.78 --locked \
 && rustup target add x86_64-unknown-linux-musl \
 && apt-get update \
 && apt-get install -y --no-install-recommends musl-tools \
 && rm -rf /var/lib/apt/lists/*

FROM chef AS planner
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
# `sdk/rust` is a member of the workspace: without its manifest, neither the
# recipe nor the build can read the workspace at all.
COPY sdk/rust/ sdk/rust/
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --locked --target x86_64-unknown-linux-musl --recipe-path recipe.json
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
COPY sdk/rust/ sdk/rust/
RUN cargo build --release --locked --target x86_64-unknown-linux-musl -p telmoni-server

FROM gcr.io/distroless/static-debian13:nonroot@sha256:18f4cc26c9ffa7f20a359298be7deadac5c607c8e03a02b8119f1a5fc2f82c99
COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/telmoni-server /app/telmoni-server
# `telmoni-server migrate` reads /app/migrations/<schema>, every directory there,
# `audit` first — every module's `emit_audit` needs `audit.events` to exist.
# An image built FROM this one adds a set by copying it in beside these, and
# its grants as one file under /app/grants.
COPY --from=builder /app/crates/migrator/migrations      /app/migrations/audit
COPY --from=builder /app/crates/auth/migrations          /app/migrations/auth
COPY --from=builder /app/crates/notifications/migrations /app/migrations/notifications
COPY --from=builder /app/crates/agent/migrations         /app/migrations/agent
COPY --from=builder /app/crates/telemetry/migrations     /app/migrations/telemetry
EXPOSE 8082
ENTRYPOINT ["/app/telmoni-server"]
CMD ["serve"]
