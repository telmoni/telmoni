// What a new key's dialog offers to paste: per SDK, one shell block that
// installs it, sets the key, and sends one run, so the first trace is a
// paste away. Each runs in a subshell in a new directory, so a second paste
// touches nothing of the reader's and the key outlives it nowhere. Each is
// the book's smallest run (`api/sdks.mdx`, *A run*); the Rust one drops its
// handles, which ends them whichever release is installed.

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
with telmoni.run("my-agent"):
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
telmoni.run('my-agent', (run) => run.step('hello', () => {}));
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
    ctx, run := client.Run(context.Background(), "my-agent", telmoni.RunOptions{})
    _, step := run.Step(ctx, "hello")
    step.End()
    run.End()
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
    let run = telmoni.run("my-agent", telmoni::RunOptions::default())?;
    drop(run.step("hello"));
    drop(run);
    telmoni.flush();
    Ok(())
}
EOF
cargo run --quiet`),
    },
  ];
}
