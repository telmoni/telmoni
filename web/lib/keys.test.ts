import { describe, expect, it } from "vitest";

import { buildConsoleNav } from "./console-nav";
import { GO_SEQUENCES, PAGE_ACTION_KEY, resolveSequence } from "./keys";

describe("the go-sequence registry", () => {
  it("gives every rail destination a sequence", () => {
    for (const item of buildConsoleNav("/prj-123/api-keys", "prj-123").flatMap((g) => g.items)) {
      expect(
        GO_SEQUENCES.some((s) => resolveSequence(s.key, "prj-123") === item.url),
        `no sequence reaches ${item.title} (${item.url})`,
      ).toBe(true);
    }
  });

  it("resolves every sequence to a live destination", () => {
    const OFF_RAIL = new Set(["/organization", "/prj-123/audit-log"]);
    const urls = new Set(
      buildConsoleNav("/prj-123/api-keys", "prj-123")
        .flatMap((g) => g.items)
        .map((i) => i.url),
    );
    for (const s of GO_SEQUENCES) {
      const href = resolveSequence(s.key, "prj-123");
      expect(
        href !== null && (urls.has(href) || OFF_RAIL.has(href)),
        `g ${s.key} resolves nowhere: ${href}`,
      ).toBe(true);
    }
  });

  it("reaches every page from anywhere", () => {
    expect(resolveSequence("k", "prj-123")).toBe("/prj-123/api-keys");
    expect(resolveSequence("c", "prj-123")).toBe("/prj-123/connectors");
    expect(resolveSequence("u", "prj-123")).toBeNull();
    expect(resolveSequence("u")).toBeNull();
    expect(resolveSequence("O", "prj-123")).toBe("/organization");
    expect(resolveSequence("O", "organization")).toBe("/organization");
    expect(resolveSequence("O")).toBe("/organization");
  });

  it("keeps every key single, unique, and off the reserved starters", () => {
    const keys = GO_SEQUENCES.map((s) => s.key);
    expect(new Set(keys).size).toBe(keys.length);
    for (const k of keys) expect(k.length).toBe(1);
    expect(PAGE_ACTION_KEY.length).toBe(1);
    expect(["g", "?"].includes(PAGE_ACTION_KEY)).toBe(false);
  });

  // ⚠ The untested corner that shipped a 404. With no root the function used
  // to hand back the bare project-relative href — `/api-keys`, `/members` — and those
  // match `[projectId]` with the page name as the project id.
  it.each(["k", "c", "m", "l", "p", "o"])(
    "refuses the project-relative %s when there is no project root",
    (k) => {
      expect(resolveSequence(k)).toBeNull();
    },
  );

  it.each(["account", "console"])(
    "refuses a project-relative sequence under the non-project root %s",
    (root) => {
      expect(resolveSequence("k", root)).toBeNull();
      expect(resolveSequence("o", root)).toBeNull();
      expect(resolveSequence("O", root)).toBe("/organization");
    },
  );

  it("does nothing with no root at all", () => {
    expect(resolveSequence("k", null)).toBeNull();
    expect(resolveSequence("k", undefined)).toBeNull();
  });

  it("no-ops an unknown second key", () => {
    expect(resolveSequence("z", "prj-123")).toBeNull();
    expect(resolveSequence("x", "prj-123")).toBeNull();
    expect(resolveSequence("X", "prj-123")).toBeNull();
    expect(resolveSequence("O", "prj-123")).toBe("/organization");
    expect(resolveSequence("k", "organization")).toBeNull();
    expect(resolveSequence("c", "organization")).toBeNull();
    expect(resolveSequence("u", "organization")).toBeNull();
    expect(resolveSequence("m", "organization")).toBe("/organization/members");
    expect(resolveSequence("l", "organization")).toBe("/organization/audit-log");
    expect(resolveSequence("p", "organization")).toBe("/organization/settings");
    expect(resolveSequence("o", "organization")).toBe("/organization");
  });
});
