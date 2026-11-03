// What a list's first-run page offers to paste: where each SDK names a run's
// session or its user, as the book's *A run* names them (`api/sdks.mdx`), and
// the attribute another exporter sets.

export type Snippet = { label: string; command: string };

type Field = "session" | "user";

const FIELD = {
  session: { python: "session_id", ts: "sessionId", go: "SessionID", rust: "session_id", otel: "gen_ai.conversation.id", value: "conv_7" },
  user: { python: "user_id", ts: "userId", go: "UserID", rust: "user_id", otel: "user.id", value: "user_42" },
} as const;

/** Each SDK's run naming its session or its user, and the attribute for any other exporter. */
export function fieldSnippets(field: Field): Snippet[] {
  const f = FIELD[field];
  return [
    { label: "Python", command: `with telmoni.run("support-bot", ${f.python}="${f.value}"):\n    ...` },
    {
      label: "TypeScript",
      command: `await telmoni.run('support-bot', { ${f.ts}: '${f.value}' }, async (run) => {\n  // ...\n});`,
    },
    {
      label: "Go",
      command: `ctx, run := client.Run(ctx, "support-bot", telmoni.RunOptions{${f.go}: "${f.value}"})\ndefer run.End()`,
    },
    {
      label: "Rust",
      command: `let run = telmoni.run(\n    "support-bot",\n    RunOptions { ${f.rust}: Some("${f.value}".into()), ..RunOptions::default() },\n)?;`,
    },
    { label: "OpenTelemetry", command: `span.set_attribute("${f.otel}", "${f.value}")` },
  ];
}

/** What a coding agent is asked to paste-run, to instrument the project it works in. */
export const AGENT_PROMPT =
  "Install the telmoni skill from github.com/telmoni/skills, then use it to record this application's agent runs in Telmoni with the SDK for its language.";
