// What a new key's dialog offers to paste: per SDK, one shell block that
// installs it, sets the key, and sends one trace, so the first trace is a
// paste away. Each runs in a subshell in a new directory, so a second paste
// touches nothing of the reader's and the key outlives it nowhere. Each is
// the book's smallest trace (`api/sdks.mdx`, *A trace*); the Rust one drops
// its handles, which ends them whichever release is installed.

export type Snippet = { label: string; command: string };

export function firstTraceSnippets(key: string): Snippet[] {
  const env = `export TELMONI_API_KEY=${key}`;
  const block = (lines: string) => `( set -e; cd "$(mktemp -d)"
${lines}
)`;
  return [
    {
      label: "Python",
      command: block(`python3 -m venv .venv && . .venv/bin/activate
pip install telmoni
${env}
python - <<'EOF'
from telmoni import Telmoni

telmoni = Telmoni.from_env()
with telmoni.trace("my-agent"):
    with telmoni.step("hello"):
        pass
telmoni.flush()
EOF`),
    },
    {
      label: "TypeScript",
      command: block(`npm init -y >/dev/null
npm install telmoni
${env}
cat > first-trace.mjs <<'EOF'
import { Telmoni } from 'telmoni';

const telmoni = Telmoni.fromEnv();
telmoni.trace('my-agent', (trace) => trace.step('hello', () => {}));
await telmoni.flush();
EOF
node first-trace.mjs`),
    },
    {
      label: "Go",
      command: block(`go mod init first-trace
go get github.com/telmoni/telmoni/sdk/go
${env}
cat > main.go <<'EOF'
package main

import (
    "context"
    "log"

    telmoni "github.com/telmoni/telmoni/sdk/go"
)

func main() {
    client, err := telmoni.NewFromEnv()
    if err != nil {
        log.Fatal(err)
    }
    defer client.Shutdown(context.Background())
    ctx, trace := client.Trace(context.Background(), "my-agent", telmoni.TraceOptions{})
    _, step := trace.Step(ctx, "hello")
    step.End()
    trace.End()
}
EOF
go run .`),
    },
    {
      label: "Rust",
      command: block(`cargo new --quiet first-trace && cd first-trace
cargo add telmoni
${env}
cat > src/main.rs <<'EOF'
fn main() -> Result<(), telmoni::Error> {
    let telmoni = telmoni::Telmoni::from_env();
    let trace = telmoni.trace("my-agent", telmoni::TraceOptions::default())?;
    drop(trace.step("hello"));
    drop(trace);
    telmoni.flush();
    Ok(())
}
EOF
cargo run --quiet`),
    },
  ];
}
