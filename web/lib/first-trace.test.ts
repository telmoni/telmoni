import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { firstTraceSnippets } from "./first-trace";

const KEY = "tm_0123456789abcdefghijklmnopqrstuvwxyzABCDEFG";

/** Each SDK's install command as the book's *Installation* prints it. */
function bookInstalls(): Map<string, string> {
  const page = readFileSync(join(__dirname, "../content/docs/api/sdks.mdx"), "utf8");
  const section = page.slice(page.indexOf("## Installation"), page.indexOf("## A run"));
  const installs = new Map<string, string>();
  for (const [, label, command] of section.matchAll(/<Tab value="(\w+)">\s*```console\s*\$ ([^\n]+)/g)) {
    installs.set(label!, command!.trim());
  }
  return installs;
}

describe("firstTraceSnippets", () => {
  it("installs each SDK as the book does, sets the key once, and runs one run", () => {
    const installs = bookInstalls();
    const snippets = firstTraceSnippets(KEY);
    expect(snippets.map((s) => s.label)).toEqual([...installs.keys()]);
    for (const { label, command } of snippets) {
      const lines = command.split("\n");
      expect(lines, label).toContain(installs.get(label));
      expect(lines.filter((line) => line.includes(KEY)), label).toEqual([
        `export TELMONI_API_KEY=${KEY}`,
      ]);
      expect(command, label).toMatch(/["']my-agent["']/);
    }
  });
});
