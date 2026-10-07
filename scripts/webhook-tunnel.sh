#!/usr/bin/env bash
# A public HTTPS door to a receiver on this laptop, so an outbound webhook can
# be tested end to end from `make up`. Dev tooling only: nothing here is
# read by a service or by Terraform, so a deployed tier cannot pick it up.
#
# Why a tunnel at all: the outbound guard refuses any endpoint that is
# loopback, private or the metadata server — checked at registration, at dial
# time and at DNS resolution, with no development override on purpose. A
# Cloudflare quick tunnel hands out an `https://…trycloudflare.com` hostname
# that resolves to a public address, which is exactly what a customer's
# endpoint looks like to the guard.
#
#   make webhook-tunnel                        # receiver + tunnel, prints the URL
#   TELMONI_WEBHOOK_SECRET=whsec_… make webhook-tunnel   # once the secret exists
#
# The URL changes every run (quick tunnels are anonymous and unnamed); the
# same URL cannot be connected twice to one project, so reconnect after a
# restart rather than expecting the old row to come back to life. Once a named
# route forwards a stable hostname to this machine, use `make webhook-receiver`
# instead — same receiver, without a second URL to reconnect.
set -euo pipefail

PORT="${PORT:-9000}"
HOOK_PATH="${HOOK_PATH:-/hook}"

if ! command -v cloudflared >/dev/null 2>&1; then
  echo "✗ cloudflared is not installed.  Install: brew install cloudflared  (macOS)" >&2
  echo "                                            https://developers.cloudflare.com/cloudflare-one/connections/connect-networks/downloads/" >&2
  exit 1
fi
if ! command -v node >/dev/null 2>&1; then
  echo "✗ node is not installed; the receiver is a Node script." >&2
  exit 1
fi

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
log="$(mktemp -t telmoni-tunnel.XXXXXX)"

cleanup() {
  # Both children, whichever order they started in; a stray receiver holding
  # the port is what makes the NEXT run fail with EADDRINUSE.
  [ -n "${receiver_pid:-}" ] && kill "$receiver_pid" 2>/dev/null || true
  [ -n "${tunnel_pid:-}" ] && kill "$tunnel_pid" 2>/dev/null || true
  rm -f "$log"
}
trap cleanup EXIT INT TERM

PORT="$PORT" node "$here/webhook-receiver.mjs" &
receiver_pid=$!

# cloudflared prints the assigned hostname to stderr a second or two after
# start, inside a box of dashes. Tee it so the operator sees cloudflared's own
# output, and read the URL out of the same stream to print it once, plainly,
# with the path the endpoint should carry. Process substitution rather than a
# pipe, so `$!` is cloudflared's pid and not tee's: killed by pid, a tee leaves
# the tunnel open until cloudflared next writes to the closed pipe.
cloudflared tunnel --url "http://localhost:${PORT}" > >(tee "$log") 2>&1 &
tunnel_pid=$!

url=""
for _ in $(seq 1 40); do
  url="$(grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' "$log" | head -n1 || true)"
  [ -n "$url" ] && break
  sleep 0.5
done

if [ -z "$url" ]; then
  echo "✗ cloudflared did not report a hostname within 20s; its output is above." >&2
  exit 1
fi

cat <<EOF

────────────────────────────────────────────────────────────────────────────
  Endpoint to paste on the Connectors page:

      ${url}${HOOK_PATH}

  Then copy the whsec_ secret it answers with and restart this with it set:

      TELMONI_WEBHOOK_SECRET=whsec_… make webhook-tunnel

  The first notice arrives the moment you connect — before the secret is
  here — so the receiver answers 401 and the loop retries in 30s. Or press
  Test on the connection once the secret is set. Ctrl-C stops both.
────────────────────────────────────────────────────────────────────────────

EOF

wait "$tunnel_pid"
