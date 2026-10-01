import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

// ⚠ The console used to state who was asking through these headers, signed
// with a key that could therefore act as anyone. The session's bearer replaced
// them: a service reads the person from the token and from nothing else, so a
// header spelt here again would be sent, ignored, and mistaken by the next
// reader for a mechanism. Source only — a test may name one to assert its
// absence, as `identity-context.test.ts` does.
const WEB = path.resolve(__dirname, "../..");
const ROOTS = ["app", "lib"].map((dir) => path.join(WEB, dir));
const RETIRED = ["x-user-id", "x-role", "x-identity-signature", "x-identity-expires"];

function sourceFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      if (entry.name !== "node_modules") sourceFiles(full, out);
      continue;
    }
    if (!/\.tsx?$/.test(entry.name) || /\.test\.tsx?$/.test(entry.name)) continue;
    out.push(full);
  }
  return out;
}

const files = ROOTS.flatMap((root) => sourceFiles(root));

describe("the retired identity headers", () => {
  it("scanned the console's source at all, so a pass is not vacuous", () => {
    expect(files.length).toBeGreaterThan(50);
    expect(
      files.some((f) => f.endsWith(path.join("entities", "identity-context.ts"))),
      "the module that builds every outbound header set is missing from the scan",
    ).toBe(true);
  });

  it.each(RETIRED)("%s is spelt nowhere under web/app or web/lib", (header) => {
    const hits = files
      .filter((f) => readFileSync(f, "utf8").includes(header))
      .map((f) => path.relative(WEB, f));
    expect(hits).toEqual([]);
  });
});
