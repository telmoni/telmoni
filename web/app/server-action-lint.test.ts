import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { ESLint } from "eslint";
import { describe, expect, it } from "vitest";

// Every Server Action is an outbound-call site and a public endpoint, so the
// bare-`fetch` ban has to reach all of them. It did not: the glob was
// `app/**/actions.ts`, which matches the name literally, and four of the
// twelve action files are spelled `<thing>-actions.ts`.
//
// ESLint resolves this itself rather than the test re-reading the globs — the
// bug WAS a glob read one way by its author and another by minimatch.
const WEB = path.join(__dirname, "..");
const BARE_FETCH = "Use fetchWithTimeout()";

function serverActionFiles(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      serverActionFiles(full, out);
      continue;
    }
    if (!/\.tsx?$/.test(entry.name) || /\.test\.tsx?$/.test(entry.name)) continue;
    if (/^\s*["']use server["']/.test(readFileSync(full, "utf8"))) out.push(full);
  }
  return out;
}

const files = serverActionFiles(__dirname);

describe("the bare-fetch ban reaches every Server Action", () => {
  it("found the action files at all — a zero here would pass vacuously", () => {
    expect(files.length).toBeGreaterThanOrEqual(12);
    expect(
      files.some((f) => path.basename(f).match(/^.+-actions\.ts$/)),
      "no *-actions.ts file in the scan, so the regression it guards is unreachable",
    ).toBe(true);
  });

  // The first case loads ESLint's config, plugins and parser: about three
  // seconds alone, past the default five while the whole suite runs at once.
  it.each(files.map((f) => path.relative(WEB, f)))("covers %s", { timeout: 30_000 }, async (rel) => {
    const eslint = new ESLint({ cwd: WEB });
    const config = (await eslint.calculateConfigForFile(rel)) as {
      rules?: Record<string, unknown[]>;
    };
    const restricted = config.rules?.["no-restricted-syntax"] ?? [];
    const messages = restricted
      .filter((o): o is { message?: string } => typeof o === "object" && o !== null)
      .map((o) => o.message ?? "");
    expect(
      messages.some((m) => m.startsWith(BARE_FETCH)),
      `${rel} is outside the bare-fetch rule's file globs`,
    ).toBe(true);
  });
});
