# syntax=docker/dockerfile:1.7
# Next.js (standalone) image for the Telmoni web app.
#
# Build from the web/ directory — publish.yml sets the context to web/:
#   docker build -f web/Dockerfile -t telmoni-web web/
#
# `output: "standalone"` in next.config.mjs emits a self-contained bundle that
# runs with `node server.js` — no `next start`, no full node_modules.
#
# Required env at runtime (see web/.env.local.example):
#   AUTH_SECRET, AUTH_URL, SERVICE_SECRET, SERVER_URL, REDIS_URL
#   Optional: REDIS_CA_CERT (only for a rediss:// URL).
# No identity-provider credentials — web delegates all of that to the
# server; AUTH_PROVIDER_ORIGINS only lets the browser follow its redirects.

# ⚠ THE BUILD STAGES AND THE RUNTIME MUST SHARE A NODE MAJOR, and the runtime
# pins it: `gcr.io/distroless/nodejsNN-debian13` publishes 22, 24 and 26 only. A
# mismatch builds and runs until newer output meets the older runtime, and the
# failure lands in a distroless image with no shell to open.
FROM node:24-bookworm-slim@sha256:55fb5f5a527ea2c5c4a95f963e48ef90aa38c7b346054fdc8ee9957a2b55d205 AS deps
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci

# ── Stage 2: build the standalone bundle ─────────────────────────────────────
FROM node:24-bookworm-slim@sha256:55fb5f5a527ea2c5c4a95f963e48ef90aa38c7b346054fdc8ee9957a2b55d205 AS builder
WORKDIR /app
# CA roots for any HTTPS the build performs (e.g. next/font fetches).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=deps /app/node_modules ./node_modules
COPY . .
# No origin is baked in: the console reads AUTH_URL at request time, so the
# one published image serves every deployment. A `NEXT_PUBLIC_` variable here
# would be inlined into the bundle and name the build machine's host.
ENV NEXT_TELEMETRY_DISABLED=1
RUN npm run build
# ⚠ Next writes its ISR cache here at RUNTIME, but the runtime stage is
# distroless (no shell, so no `RUN mkdir`) and nonroot. Without this every
# cacheable render hits EACCES and silently re-renders instead.
RUN mkdir -p /app/.next/cache

# ── Stage 3: minimal runtime — distroless node, non-root ─────────────────────
FROM gcr.io/distroless/nodejs24-debian13:nonroot@sha256:cb606325b7ec4af55462cb0b424a53b452a5b4a894ffe841fcf1048d216fb9f8 AS runner
WORKDIR /app
ENV NODE_ENV=production \
    NEXT_TELEMETRY_DISABLED=1 \
    PORT=3000 \
    HOSTNAME=0.0.0.0
# Standalone layout: server.js + minimal node_modules at the root, static
# assets under .next/static, public/ alongside.
COPY --from=builder /app/.next/standalone ./
COPY --from=builder /app/.next/static ./.next/static
COPY --from=builder /app/public ./public
# The one path the server writes to. distroless `nonroot` is uid/gid 65532;
# --chown needs numerics, as the builder has no /etc/passwd to look it up in.
COPY --from=builder --chown=65532:65532 /app/.next/cache ./.next/cache
USER nonroot
EXPOSE 3000
# distroless/nodejs ENTRYPOINT is `node`; CMD supplies the script.
CMD ["server.js"]
