import { describe, expect, it } from "vitest";

import { buildConsoleNav } from "./console-nav";
import { GO_SEQUENCES, PAGE_ACTION_KEY, resolveSequence } from "./keys";

describe("the go-sequence registry", () => {
  it("gives every rail destination a sequence", () => {
    for (const item of buildConsoleNav("/acme/web/api-keys").flatMap((g) => g.items)) {
      expect(
        GO_SEQUENCES.some((s) => resolveSequence(s.key, "/acme/web/api-keys") === item.url),
        `no sequence reaches ${item.title} (${item.url})`,
      ).toBe(true);
    }
  });

  it("resolves every sequence to a live destination", () => {
    const OFF_RAIL = new Set(["/acme"]);
    const urls = new Set(
      buildConsoleNav("/acme/web/api-keys")
        .flatMap((g) => g.items)
        .map((i) => i.url),
    );
    for (const s of GO_SEQUENCES) {
      const href = resolveSequence(s.key, "/acme/web/api-keys");
      expect(
        href !== null && (urls.has(href) || OFF_RAIL.has(href)),
        `g ${s.key} resolves nowhere: ${href}`,
      ).toBe(true);
    }
  });

  it("reaches every page from anywhere", () => {
    expect(resolveSequence("k", "/acme/web")).toBe("/acme/web/api-keys");
    expect(resolveSequence("c", "/acme/web/members")).toBe("/acme/web/connectors");
    expect(resolveSequence("u", "/acme/web")).toBeNull();
    expect(resolveSequence("u", "/")).toBeNull();
    expect(resolveSequence("O", "/acme/web")).toBe("/acme");
    expect(resolveSequence("O", "/acme/~/settings")).toBe("/acme");
    expect(resolveSequence("O", "/account/settings", "acme")).toBe("/acme");
  });

  // The path names the organization, and wins over the one handed in: that is
  // only for a path that names none.
  it("goes to the organization the path names", () => {
    expect(resolveSequence("O", "/globex/web", "acme")).toBe("/globex");
    expect(resolveSequence("m", "/globex/~/settings", "acme")).toBe("/globex/~/members");
  });

  it("keeps every key single, unique, and off the reserved starters", () => {
    const keys = GO_SEQUENCES.map((s) => s.key);
    expect(new Set(keys).size).toBe(keys.length);
    for (const k of keys) expect(k.length).toBe(1);
    expect(PAGE_ACTION_KEY.length).toBe(1);
    expect(["g", "?"].includes(PAGE_ACTION_KEY)).toBe(false);
  });

  // ⚠ The untested corner that shipped a 404. With nowhere to stand the
  // function used to hand back the bare project-relative href — `/api-keys`,
  // `/members` — and those read as an organization of that name.
  it.each(["k", "c", "m", "l", "p", "o"])(
    "refuses the project-relative %s when the path stands nowhere",
    (k) => {
      expect(resolveSequence(k, "/")).toBeNull();
      expect(resolveSequence(k, "/", "acme")).toBeNull();
    },
  );

  it.each(["/account/settings", "/console"])(
    "refuses a project-relative sequence on %s",
    (pathname) => {
      expect(resolveSequence("k", pathname, "acme")).toBeNull();
      expect(resolveSequence("o", pathname, "acme")).toBeNull();
      expect(resolveSequence("O", pathname, "acme")).toBe("/acme");
    },
  );

  it("does nothing with no organization to go to", () => {
    expect(resolveSequence("O", "/account/settings")).toBeNull();
    expect(resolveSequence("O", "/account/settings", null)).toBeNull();
    expect(resolveSequence("k", "/account/settings", null)).toBeNull();
  });

  it("no-ops an unknown second key", () => {
    expect(resolveSequence("z", "/acme/web")).toBeNull();
    expect(resolveSequence("x", "/acme/web")).toBeNull();
    expect(resolveSequence("X", "/acme/web")).toBeNull();
    expect(resolveSequence("O", "/acme/web")).toBe("/acme");
    expect(resolveSequence("k", "/acme/~/members")).toBeNull();
    expect(resolveSequence("c", "/acme")).toBeNull();
    expect(resolveSequence("u", "/acme")).toBeNull();
    expect(resolveSequence("m", "/acme")).toBe("/acme/~/members");
    expect(resolveSequence("l", "/acme/~/members")).toBe("/acme/~/audit-log");
    expect(resolveSequence("p", "/acme")).toBe("/acme/~/settings");
    expect(resolveSequence("o", "/acme/~/settings")).toBe("/acme");
  });
});
