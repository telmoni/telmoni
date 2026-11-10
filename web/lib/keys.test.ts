import { describe, expect, it } from "vitest";

import { buildConsoleNav } from "./console-nav";
import {
  NOTIFICATIONS_KEY,
  ORGANIZATION_KEY,
  PAGE_ACTION_KEY,
  goSequences,
  pressedKey,
  resolveSequence,
  sequenceHint,
} from "./keys";

// One path on each rail: a project's, an organization's, the person's.
const RAILS = ["/acme/web/api-keys", "/acme/settings", "/settings/account"];

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
        if (s.key === ORGANIZATION_KEY || s.key === NOTIFICATIONS_KEY) continue;
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
    expect(resolveSequence("o", "/acme/web/members")).toBe("/acme/web");
    expect(resolveSequence("t", "/acme/web/members")).toBe("/acme/web/traces");
    expect(resolveSequence("a", "/acme/web")).toBe("/acme/web/alerts");
    expect(resolveSequence("v", "/acme/web")).toBe("/acme/web/objectives");
    expect(resolveSequence("s", "/acme/web")).toBe("/acme/web/dashboards");
    expect(resolveSequence(",", "/acme/web")).toBe("/acme/web/settings");
    expect(resolveSequence("p", "/acme/web")).toBeNull();
    expect(resolveSequence("k", "/acme/members")).toBeNull();
    expect(resolveSequence("p", "/acme")).toBe("/acme/projects");
    expect(resolveSequence("m", "/acme")).toBe("/acme/members");
    expect(resolveSequence(",", "/acme/members")).toBe("/acme/settings");
    expect(resolveSequence(",", "/settings/account")).toBe("/settings/general");
    expect(resolveSequence("u", "/settings/general")).toBe("/settings/account");
    expect(resolveSequence("n", "/settings/account")).toBe("/settings/notifications");
    expect(resolveSequence("p", "/settings/account")).toBe("/settings/privacy");
    expect(resolveSequence("a", "/settings/privacy")).toBe("/settings/accessibility");
    expect(resolveSequence("z", "/acme/web")).toBeNull();
    expect(resolveSequence("X", "/acme/web")).toBeNull();
  });

  // The path names the organization, and wins over the one handed in: that is
  // only for a path that names none.
  it("goes to the organization's projects from anywhere, the path's first", () => {
    expect(resolveSequence("O", "/acme/web")).toBe("/acme/projects");
    expect(resolveSequence("O", "/acme/settings")).toBe("/acme/projects");
    expect(resolveSequence("O", "/globex/web", "acme")).toBe("/globex/projects");
    expect(resolveSequence("O", "/settings/account", "acme")).toBe("/acme/projects");
    expect(resolveSequence("O", "/console", "acme")).toBe("/acme/projects");
    expect(resolveSequence("O", "/settings/account")).toBeNull();
    expect(resolveSequence("O", "/settings/account", null)).toBeNull();
  });

  // The rail's own row where it has one, a project's or an organization's;
  // the person's own from Settings' rail and off the console's paths.
  it("goes to the notifications of the place it stands in, once a rail", () => {
    for (const [pathname, href] of [
      ["/acme/web", "/acme/web/notifications"],
      ["/acme/web/traces", "/acme/web/notifications"],
      ["/acme", "/acme/notifications"],
      ["/acme/members", "/acme/notifications"],
      ["/settings/account", "/settings/notifications"],
    ] as const) {
      expect(resolveSequence("n", pathname, "acme"), pathname).toBe(href);
      expect(goSequences(pathname, "acme").filter((s) => s.key === "n"), pathname).toEqual([
        { key: "n", label: "Notifications", href },
      ]);
    }
  });

  // Spend took `s` from Dashboards' `d`: the old letter leads nowhere, so a
  // hand that remembers it is not sent astray.
  it("retires g d on a project's rail", () => {
    expect(resolveSequence("d", "/acme/web")).toBeNull();
  });

  it("names a sequence in a tooltip as the sheet's keys read", () => {
    expect(sequenceHint("t")).toBe("G then T");
    expect(sequenceHint(NOTIFICATIONS_KEY)).toBe("G then N");
    expect(sequenceHint(",")).toBe("G then ,");
    expect(sequenceHint(ORGANIZATION_KEY)).toBe("G then ⇧ O");
  });

  // ⚠ The corner that once shipped a 404: with nowhere to stand, a bare
  // project-relative href read as an organization of that name. Off the
  // console's paths the rail is empty, so nothing but `g O` resolves.
  it.each(["k", "c", "m", "l", ",", "o", "t", "p", "n"])("refuses %s where no rail stands", (k) => {
    expect(resolveSequence(k, "/")).toBeNull();
    expect(resolveSequence(k, "/", "acme")).toBeNull();
    expect(resolveSequence(k, "/console", "acme")).toBeNull();
  });
});

// The chords once threw on a plain Event dispatched under the keydown name,
// as a password manager sends one, since it carries no key at all.
describe("the pressed key", () => {
  it("is the KeyboardEvent's key, as typed", () => {
    expect(pressedKey({ key: "K" } as KeyboardEvent)).toBe("K");
  });

  it("is nothing for an event that names none", () => {
    expect(pressedKey(new Event("keydown") as KeyboardEvent)).toBe("");
  });
});
