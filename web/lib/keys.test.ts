import { describe, expect, it } from "vitest";

import { buildConsoleNav } from "./console-nav";
import { ORGANIZATION_KEY, PAGE_ACTION_KEY, goSequences, resolveSequence } from "./keys";

// One path on each rail: a project's, an organization's, the person's.
const RAILS = ["/acme/web/api-keys", "/acme/settings", "/account/settings"];

describe("the go sequences", () => {
  it("reaches every row of every rail, and promises nothing the rail lacks", () => {
    for (const pathname of RAILS) {
      const rows = buildConsoleNav(pathname).flatMap((g) => g.items);
      for (const row of rows) {
        expect(row.key, `${row.title} has no key on ${pathname}`).toBeDefined();
        expect(resolveSequence(row.key ?? "", pathname, "acme"), `g ${row.key} on ${pathname}`).toBe(
          row.url,
        );
      }
      const urls = new Set(rows.map((r) => r.url));
      for (const s of goSequences(pathname, "acme")) {
        if (s.key === ORGANIZATION_KEY) continue;
        expect(urls.has(s.href), `g ${s.key} promises ${s.href} off the rail`).toBe(true);
      }
    }
  });

  it("keeps every key single and unique within a rail, and off the reserved starters", () => {
    for (const pathname of RAILS) {
      const keys = goSequences(pathname, "acme").map((s) => s.key);
      expect(new Set(keys).size, pathname).toBe(keys.length);
      for (const k of keys) {
        expect(k.length).toBe(1);
        expect(["g", "?"].includes(k)).toBe(false);
      }
    }
    expect(PAGE_ACTION_KEY.length).toBe(1);
    expect(["g", "?"].includes(PAGE_ACTION_KEY)).toBe(false);
  });

  it("reads the rail you stand in", () => {
    expect(resolveSequence("k", "/acme/web")).toBe("/acme/web/api-keys");
    expect(resolveSequence("t", "/acme/web/members")).toBe("/acme/web/traces");
    expect(resolveSequence("d", "/acme/web")).toBe("/acme/web/dashboards");
    expect(resolveSequence(",", "/acme/web")).toBe("/acme/web/settings");
    expect(resolveSequence("p", "/acme/web")).toBeNull();
    expect(resolveSequence("k", "/acme/members")).toBeNull();
    expect(resolveSequence("p", "/acme")).toBe("/acme/projects");
    expect(resolveSequence("m", "/acme")).toBe("/acme/members");
    expect(resolveSequence(",", "/acme/members")).toBe("/acme/settings");
    expect(resolveSequence(",", "/account/settings")).toBe("/account/settings");
    expect(resolveSequence("n", "/account/settings")).toBe("/account/notifications");
    expect(resolveSequence("p", "/account/settings")).toBe("/account/privacy");
    expect(resolveSequence("a", "/account/privacy")).toBe("/account/accessibility");
    expect(resolveSequence("z", "/acme/web")).toBeNull();
    expect(resolveSequence("X", "/acme/web")).toBeNull();
  });

  // The path names the organization, and wins over the one handed in: that is
  // only for a path that names none.
  it("goes to the organization's overview from anywhere, the path's first", () => {
    expect(resolveSequence("O", "/acme/web")).toBe("/acme");
    expect(resolveSequence("O", "/acme/settings")).toBe("/acme");
    expect(resolveSequence("O", "/globex/web", "acme")).toBe("/globex");
    expect(resolveSequence("O", "/account/settings", "acme")).toBe("/acme");
    expect(resolveSequence("O", "/console", "acme")).toBe("/acme");
    expect(resolveSequence("O", "/account/settings")).toBeNull();
    expect(resolveSequence("O", "/account/settings", null)).toBeNull();
  });

  // ⚠ The corner that once shipped a 404: with nowhere to stand, a bare
  // project-relative href read as an organization of that name. Off the
  // console's paths the rail is empty, so nothing but `g O` resolves.
  it.each(["k", "c", "m", "l", ",", "o", "t", "p"])("refuses %s where no rail stands", (k) => {
    expect(resolveSequence(k, "/")).toBeNull();
    expect(resolveSequence(k, "/", "acme")).toBeNull();
    expect(resolveSequence(k, "/console", "acme")).toBeNull();
  });
});
