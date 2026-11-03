import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { fieldSnippets } from "./first-run";

const book = readFileSync(join(__dirname, "../content/docs/api/sdks.mdx"), "utf8");

describe("fieldSnippets", () => {
  it.each([
    ["session", ['session_id="conv_7"', "sessionId: 'conv_7'", 'SessionID: "conv_7"', 'session_id: Some("conv_7".into())']],
    ["user", ['user_id="user_42"', "userId: 'user_42'", 'UserID: "user_42"', 'user_id: Some("user_42".into())']],
  ] as const)("names a run's %s as each SDK's own example does", (field, spelled) => {
    const snippets = fieldSnippets(field);
    expect(snippets.map((s) => s.label)).toEqual(["Python", "TypeScript", "Go", "Rust", "OpenTelemetry"]);
    spelled.forEach((spelling, i) => {
      expect(snippets[i]?.command).toContain(spelling);
      // The book spells it the same way, so a renamed field breaks here, not on the page.
      expect(book.replace(/\s+/g, " ")).toContain(spelling.replace(/\s+/g, " ").split(":")[0]);
    });
  });
});
