# syntax=docker/dockerfile:1.7
# Next.js (standalone) image for the Telmoni web app.
#
# Build from the web/ directory — the Makefile sets the context to $(WEB_DIR):
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
FROM node:24-bookworm-slim@sha256:0e0ff40c39bc087845bfb27465a0df4ea419520094bc35842ff83dd8cbe6f9b6 AS deps
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci

# ── Stage 2: build the standalone bundle ─────────────────────────────────────
FROM node:24-bookworm-slim@sha256:0e0ff40c39bc087845bfb27465a0df4ea419520094bc35842ff83dd8cbe6f9b6 AS builder
WORKDIR /app
# CA roots for any HTTPS the build performs (e.g. next/font fetches).
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates \
 && rm -rf /var/lib/apt/lists/*
COPY --from=deps /app/node_modules ./node_modules
COPY . .
# ⚠ A build arg, not container env: `NEXT_PUBLIC_` vars are inlined into the
# client bundle at BUILD time. Without it the image ships `https://example.com`
# (web/lib/site.ts) in robots.txt, the sitemap, canonical links, OG tags and
# JSON-LD. That default is deliberate — an unconfigured build should look
# obviously wrong rather than guess a real host.
ARG NEXT_PUBLIC_APP_URL
ENV NEXT_TELEMETRY_DISABLED=1 \
    NEXT_PUBLIC_APP_URL=${NEXT_PUBLIC_APP_URL}
RUN npm run build
# ⚠ Next writes its ISR cache here at RUNTIME, but the runtime stage is
# distroless (no shell, so no `RUN mkdir`) and nonroot. Without this every
# cacheable render hits EACCES and silently re-renders instead.
RUN mkdir -p /app/.next/cache

# ── Stage 3: minimal runtime — distroless node, non-root ─────────────────────
FROM gcr.io/distroless/nodejs24-debian13:nonroot@sha256:9eeb7f5887d0e239e78264b06f7f11d2e14be534050481803a9e4728fcdd278e AS runner
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
