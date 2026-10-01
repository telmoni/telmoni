#!/usr/bin/env bash
# The server and the console in one terminal: what `make up` runs once its
# preflight has passed and the binary is built.
#
# Two processes are not worth a process manager. Each is started in the
# background and the loop below watches both: when either exits — a crash,
# or Ctrl-C reaching it — the other is stopped and this exits with the dead
# one's status. A dead server therefore never leaves a console answering
# every page with ECONNREFUSED, and the line that explains the exit is the
# last thing that process printed, right above, not under shutdown chatter.
#
# Ctrl-C reaches both processes directly, because the terminal signals the
# whole foreground group; the trap only makes sure neither is left behind,
# waits for the server to drain, and ends quietly, since stopping the stack
# is not an error. A drain that outlasts ten seconds is a hang and is killed.
#
# The ports are pinned here rather than inherited. The server's is what
# web/.env.local's SERVER_URL names and `make port-check` probes; the
# console's 3000 is what AUTH_URL / NEXT_PUBLIC_APP_URL and a provider's
# redirect URI are registered against. The server reads its DSNs and secrets
# from .env itself.
#
# Usage:   make up                        (which builds the binary first)
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT" || exit 2

BINARY=target/debug/telmoni
if [ ! -x "$BINARY" ]; then
    echo "✗ up: $BINARY is missing; \`make up\` builds it, or: cargo build -p telmoni" >&2
    exit 2
fi
SERVER_PORT="${SERVER_PORT:-8082}"

# The agent's embedding model, pulled into the compose Ollama's volume the
# first time the agent is switched on. Without it the server's width probe
# gets no answer and the agent fails every question until it does, so a
# missing model is fetched here rather than found there. Skipped when .env leaves the agent off, when the embeddings
# come from elsewhere (a hosted model has nothing to pull), or when the
# compose Ollama is not the one running (a native install pulls its own).
embeddings_url="$(sed -n 's/^EMBEDDINGS_URL=//p' .env 2>/dev/null | tail -n 1)"
if grep -qE '^AGENT_MODEL_PROVIDER=[^[:space:]]+' .env 2>/dev/null \
    && [[ -z "$embeddings_url" || "$embeddings_url" =~ ^https?://(localhost|127\.0\.0\.1):11434 ]] \
    && docker compose ps --status running --services 2>/dev/null | grep -qx ollama; then
    model="$(sed -n 's/^EMBEDDINGS_MODEL=//p' .env | tail -n 1)"
    model="${model:-nomic-embed-text}"
    if ! docker compose exec -T ollama ollama list 2>/dev/null | grep -q "^$model"; then
        echo "▶ pulling the embedding model $model into ollama (once)"
        if ! docker compose exec -T ollama ollama pull "$model"; then
            echo "✗ up: could not pull $model; the agent would answer nothing until it is there" >&2
            exit 2
        fi
    fi
fi

echo "▶ server :$SERVER_PORT, console :3000 — Ctrl-C stops both"
PORT="$SERVER_PORT" "$BINARY" serve &
server=$!
(cd web && PORT=3000 exec npm run dev) &
console=$!

stop() {
    trap '' INT TERM
    kill "$server" "$console" 2>/dev/null
    for _ in 1 2 3 4 5 6 7 8 9 10; do
        kill -0 "$server" 2>/dev/null || kill -0 "$console" 2>/dev/null || break
        sleep 1
    done
    kill -9 "$server" "$console" 2>/dev/null
    wait "$server" "$console" 2>/dev/null
}
trap 'stop; exit 0' INT
trap 'stop; exit 143' TERM

while kill -0 "$server" 2>/dev/null && kill -0 "$console" 2>/dev/null; do
    sleep 1
done

if kill -0 "$server" 2>/dev/null; then
    wait "$console"
    status=$?
    echo "✗ the console exited ($status); stopping the server." >&2
else
    wait "$server"
    status=$?
    echo "✗ the server exited ($status); stopping the console." >&2
fi
stop
exit "$status"
