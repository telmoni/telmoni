import { readdirSync, readFileSync } from "node:fs";
import path from "node:path";

import { describe, expect, it } from "vitest";

const WEB = path.join(__dirname, "..");
const APP_DIR = path.join(__dirname, "(app)");

function sources(dir: string, out: string[] = []): string[] {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) sources(full, out);
    else if (entry.name.endsWith(".tsx") && !entry.name.endsWith(".test.tsx"))
      out.push(full);
  }
  return out;
}

const files = [
  ...sources(APP_DIR),
  path.join(WEB, "components", "audit-table.tsx"),
  path.join(WEB, "components", "section.tsx"),
].map((f) => ({ file: path.relative(WEB, f), src: readFileSync(f, "utf8") }));

const dataTable = readFileSync(
  path.join(WEB, "components", "data-table.tsx"),
  "utf8",
);


describe("the console's content containers", () => {
  // globals.css gives a card containing a table `overflow-x: auto`, and a
  // scroll container that cannot be focused cannot be scrolled from a keyboard.
  // One component owns that Card now, so this reads it rather than every page —
  // but it still checks the pages, because a page that reintroduces the shell
  // by hand is exactly what would slip past.
  it("gives the card that wraps a table a keyboard way into its scroll", () => {
    const lines = dataTable.split("\n");
    const at = lines.findIndex((l) =>
      l.includes('<Card className="overflow-hidden p-0 gap-0"'),
    );
    expect(at, "data-table.tsx no longer wraps the table in the list card").toBeGreaterThanOrEqual(0);
    const next = lines.slice(at + 1).find((l) => l.trim() !== "") ?? "";
    expect(next.trim().startsWith("<table"), "the card no longer wraps a table").toBe(true);
    expect(
      lines[at],
      "the card wraps a table and cannot be reached by keyboard",
    ).toContain("tabIndex={0}");

    // Vacuous today — no page draws a `<table>` at all, and console-table.test
    // is what holds that. It is the original scan, kept because a page that
    // rebuilds the shell by hand is the one way back in, and the failure mode
    // is a card WITHOUT `tabIndex`: a string match for the correct spelling
    // would pass on exactly the source it is meant to catch. The same class
    // string wraps plenty of tableless empty-state cards, so the pairing with
    // the next line is what makes this specific.
    for (const { file, src } of files) {
      const pageLines = src.split("\n");
      pageLines.forEach((line, i) => {
        if (!line.includes('<Card className="overflow-hidden p-0 gap-0"')) return;
        const below = pageLines.slice(i + 1).find((l) => l.trim() !== "") ?? "";
        if (!below.trim().startsWith("<table")) return;
        expect(
          line,
          `${file}:${i + 1} wraps a table and cannot be reached by keyboard`,
        ).toContain("tabIndex={0}");
      });
    }
  });

});
