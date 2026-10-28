import { describe, expect, it } from "vitest";

import {
  GROUP_ORDER,
  filterItems,
  groupItems,
  listNames,
  rankItem,
  searchItems,
  unsearched,
  type SearchIndex,
  type SearchItem,
} from "./search";

const item = (
  label: string,
  extra: Partial<SearchItem> = {},
): SearchItem => ({
  id: label,
  kind: "key",
  label,
  href: "/x",
  ...extra,
});

describe("rankItem", () => {
  it("prefers the name that STARTS with what you typed", () => {
    const exact = rankItem(item("deploy"), "deploy");
    const inner = rankItem(item("staging-deploy"), "deploy");
    const mid = rankItem(item("redeployment"), "deploy");
    expect(exact).toBeLessThan(inner!);
    expect(inner).toBeLessThan(mid!);
  });

  it("matches a word inside the name, across the separators these names use", () => {
    for (const label of [
      "ci deploy",
      "ci-deploy",
      "ci_deploy",
      "ci.deploy",
      "ops@deploy",
      "acme/deploy",
    ]) {
      expect(rankItem(item(label), "deploy"), label).toBe(1);
    }
  });

  it("searches the hint, but ranks it under every name match", () => {
    expect(rankItem(item("anything", { hint: "Slack channel" }), "slack")).toBe(3);
    expect(rankItem(item("slack-relay"), "slack")).toBe(0);
  });

  it("is case-insensitive and ignores surrounding space", () => {
    expect(rankItem(item("Deploy"), "  DEPLOY ")).toBe(0);
  });

  it("answers null for a miss", () => {
    expect(rankItem(item("deploy"), "webhook")).toBeNull();
  });

  it("treats regex metacharacters as literal text", () => {
    expect(() => rankItem(item("anything"), "(")).not.toThrow();
    expect(rankItem(item("anything"), "(")).toBeNull();
    expect(rankItem(item("abc"), ".")).toBeNull();
    expect(rankItem(item("api.example"), ".")).not.toBeNull();
  });
});

describe("filterItems", () => {
  it("keeps everything, in order, for an empty query", () => {
    const items = [item("b"), item("a"), item("c")];
    expect(filterItems(items, "").map((i) => i.label)).toEqual(["b", "a", "c"]);
  });

  it("sorts by rank and leaves the caller's order inside a tier", () => {
    const items = [
      item("zeta deploy"),
      item("deploy-one"),
      item("deploy-two"),
      item("alpha deploy"),
    ];
    expect(filterItems(items, "deploy").map((i) => i.label)).toEqual([
      "deploy-one",
      "deploy-two",
      "zeta deploy",
      "alpha deploy",
    ]);
  });

  it("drops the misses", () => {
    expect(filterItems([item("a"), item("b")], "a")).toHaveLength(1);
  });
});

describe("groupItems", () => {
  it("returns the groups in GROUP_ORDER and skips the empty ones", () => {
    const items = [
      item("my key", { kind: "key" }),
      item("Payments", { kind: "project" }),
      item("Overview", { kind: "recent" }),
    ];
    expect(groupItems(items, "").map((g) => g.kind)).toEqual([
      "recent",
      "project",
      "key",
    ]);
  });

  it("puts what is about YOU first", () => {
    expect(GROUP_ORDER[0]).toBe("recent");
  });

  it("names every kind it can group", () => {
    const items = GROUP_ORDER.map((kind) => item(kind, { kind, id: kind }));
    for (const group of groupItems(items, "")) {
      expect(group.label, group.kind).toBeTruthy();
    }
    expect(groupItems(items, "")).toHaveLength(GROUP_ORDER.length);
  });
});

const INDEX: SearchIndex = {
  keys: {
    items: [item("prod key", { kind: "key", id: "key:1", href: "/acme/web/api-keys" })],
    forbidden: false,
    unavailable: false,
  },
  members: { items: [], forbidden: false, unavailable: false },
};

describe("searchItems", () => {
  const visit = { href: "/acme/web/traces", label: "Traces", hint: "kept", at: 1 };

  it("lists the recent visits placed, then the organization's projects, then the index", () => {
    const items = searchItems({
      recent: [visit],
      organization: "acme",
      projects: [{ id: "project_web", slug: "web", name: "Web" }],
      index: INDEX,
      placeOf: () => "Web",
    });
    expect(items.map((i) => [i.id, i.kind, i.hint, i.href])).toEqual([
      ["recent:/acme/web/traces", "recent", "Web", "/acme/web/traces"],
      ["project:project_web", "project", "project_web", "/acme/web"],
      ["key:1", "key", undefined, "/acme/web/api-keys"],
    ]);
  });

  it("keeps a visit's own hint where it names no place now", () => {
    const [recent] = searchItems({
      recent: [visit],
      organization: null,
      projects: [],
      index: null,
      placeOf: () => undefined,
    });
    expect(recent?.hint).toBe("kept");
  });

  it("lists no project outside an organization, and nothing of an index not read", () => {
    const items = searchItems({
      recent: [],
      organization: null,
      projects: [{ id: "project_web", slug: "web", name: "Web" }],
      index: null,
      placeOf: () => undefined,
    });
    expect(items).toEqual([]);
  });
});

describe("listNames", () => {
  it("lists names as a sentence does", () => {
    expect(listNames([])).toBe("");
    expect(listNames(["a"])).toBe("a");
    expect(listNames(["a", "b"])).toBe("a and b");
    expect(listNames(["a", "b", "c"])).toBe("a, b and c");
  });
});

describe("unsearched", () => {
  it("names nothing of an index read whole", () => {
    expect(unsearched(INDEX)).toEqual({ refused: "", broken: "" });
  });

  it("names the sections the role may not list, and the ones that could not be read", () => {
    expect(
      unsearched({
        keys: { items: [], forbidden: true, unavailable: false },
        members: { items: [], forbidden: true, unavailable: false },
      }),
    ).toEqual({ refused: "API keys and members", broken: "" });
    expect(
      unsearched({
        keys: { items: [], forbidden: false, unavailable: true },
        members: { items: [], forbidden: true, unavailable: false },
      }),
    ).toEqual({ refused: "members", broken: "API keys" });
  });
});
