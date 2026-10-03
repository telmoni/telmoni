import { describe, expect, it } from "vitest";

import {
  EMPTY_TRAIL,
  TRAIL_LIMIT,
  isReturnablePath,
  liveResources,
  parseTrail,
  pushPath,
  resolveReturnUrl,
  resourceRoot,
  resourceUrl,
  standingResource,
  trailResource,
  type ConsoleTrail,
  type Resource,
} from "./console-trail";

const ORGANIZATION: Resource = { kind: "organization", organization: "acme" };
const PROJECT: Resource = { kind: "project", organization: "acme", project: "web" };
const OTHER: Resource = { kind: "project", organization: "acme", project: "api" };
const OVERVIEW = "/acme/web";
const CONNECTORS = `${OVERVIEW}/connectors`;
const KEYS = `${OVERVIEW}/api-keys`;
const ORG_MEMBERS = "/acme/~/members";
const ACCOUNT = "/account/settings";

// What the console is drawing: one organization and its two projects.
const LIVE = liveResources(
  [{ slug: "acme" }],
  [
    { slug: "web", organizationSlug: "acme" },
    { slug: "api", organizationSlug: "acme" },
  ],
);

/** Walk the console the way the recorder does: one push per navigation. */
function walk(...paths: string[]): ConsoleTrail {
  return paths.reduce<ConsoleTrail>(pushPath, EMPTY_TRAIL);
}

describe("which paths the console may be sent back to", () => {
  it("takes any page under a project or the organization", () => {
    expect(isReturnablePath(CONNECTORS)).toBe(true);
    expect(isReturnablePath(OVERVIEW)).toBe(true);
    expect(isReturnablePath(ORG_MEMBERS)).toBe(true);
    expect(isReturnablePath("/acme")).toBe(true);
  });

  // An id is redirected to its slug, and the trail records where that lands.
  it("refuses a path that names no organization", () => {
    expect(isReturnablePath("/")).toBe(false);
    expect(isReturnablePath("/api/events")).toBe(false);
    expect(isReturnablePath("/org_7bQx2mNv9BcK4dLp/project_7bQx2mNv9BcK4dLp")).toBe(false);
  });

  it("refuses the mode the back arrow leaves", () => {
    expect(isReturnablePath("/account")).toBe(false);
    expect(isReturnablePath("/account/settings")).toBe(false);
  });

  // `/console` is a door: it asks for a name or resolves to a first page,
  // so remembering it would send you somewhere you never stood.
  it("refuses the console entry redirect", () => {
    expect(isReturnablePath("/console")).toBe(false);
  });

  it("refuses anything that could leave the origin", () => {
    expect(isReturnablePath("//evil.example.com")).toBe(false);
    expect(isReturnablePath("https://evil.example.com")).toBe(false);
    expect(isReturnablePath("/\\evil.example.com")).toBe(false);
    expect(isReturnablePath("acme/web/connectors")).toBe(false);
  });
});

describe("arriving somewhere", () => {
  it("holds one entry per resource, newest first", () => {
    expect(walk(CONNECTORS, ORG_MEMBERS)).toEqual([ORG_MEMBERS, CONNECTORS]);
  });

  // Moving around inside a resource replaces its entry rather than stacking on
  // it: this is an order of resources, not a browser history.
  it("keeps only the last page stood on in a resource", () => {
    expect(walk(CONNECTORS, KEYS)).toEqual([KEYS]);
  });

  it("re-orders a resource you come back to", () => {
    expect(walk(CONNECTORS, ORG_MEMBERS, KEYS)).toEqual([KEYS, ORG_MEMBERS]);
  });

  it("drops the oldest resource past the cap", () => {
    const trail = walk(
      ...Array.from({ length: TRAIL_LIMIT + 1 }, (_, i) => `/acme/project-${i}/api-keys`),
    );
    expect(trail).toHaveLength(TRAIL_LIMIT);
    expect(trail).not.toContain("/acme/project-0/api-keys");
  });

  it("counts an organization's overview and its own pages as one resource", () => {
    expect(walk("/acme", "/acme/~/projects", "/acme/~/billing")).toEqual(["/acme/~/billing"]);
  });

  // Two organizations may each have a project of the same name.
  it("keeps same-named projects of two organizations apart", () => {
    expect(walk("/acme/web/connectors", "/globex/web")).toEqual([
      "/globex/web",
      "/acme/web/connectors",
    ]);
    expect(trailResource("/acme/web/connectors")).toBe("/acme/web");
    expect(trailResource("/acme/~/members")).toBe("/acme");
    expect(trailResource("/account/settings")).toBe("");
  });

  // The store compares snapshots by identity, so an unchanged trail has to be
  // the SAME array, not an equal one.
  it("hands back the same trail when nothing would change", () => {
    const trail = walk(CONNECTORS);
    expect(pushPath(trail, CONNECTORS)).toBe(trail);
    expect(pushPath(trail, "/account/settings")).toBe(trail);
    expect(pushPath(trail, "//evil.example.com")).toBe(trail);
  });
});

describe("a trail read back out of storage", () => {
  it("is empty when nothing was stored", () => {
    expect(parseTrail(null)).toBe(EMPTY_TRAIL);
  });

  it("round-trips what was written", () => {
    const trail = walk(CONNECTORS, ORG_MEMBERS);
    expect(parseTrail(JSON.stringify(trail))).toEqual(trail);
  });

  it("is empty for a value that is not a trail at all", () => {
    expect(parseTrail("{oh no")).toBe(EMPTY_TRAIL);
    expect(parseTrail(JSON.stringify({ a: 1 }))).toBe(EMPTY_TRAIL);
    expect(parseTrail(JSON.stringify([1, 2]))).toBe(EMPTY_TRAIL);
  });

  // Storage is writable by anything else on the origin, and these land in hrefs.
  it("drops entries it would not have written", () => {
    expect(
      parseTrail(JSON.stringify(["//evil.example.com", "/account", CONNECTORS])),
    ).toEqual([CONNECTORS]);
  });

  it("keeps one entry per resource and applies the cap again", () => {
    expect(parseTrail(JSON.stringify([KEYS, CONNECTORS, ORG_MEMBERS]))).toEqual([
      KEYS,
      ORG_MEMBERS,
    ]);
    const long = Array.from({ length: TRAIL_LIMIT + 3 }, (_, i) => `/acme/project-${i}`);
    expect(parseTrail(JSON.stringify(long))).toHaveLength(TRAIL_LIMIT);
  });
});

describe("where the rail's back arrow points", () => {
  it("falls back when nothing was recorded", () => {
    expect(resolveReturnUrl(EMPTY_TRAIL, LIVE, OVERVIEW)).toBe(OVERVIEW);
  });

  it("returns the page you stepped in from", () => {
    expect(resolveReturnUrl(walk(CONNECTORS), LIVE, OVERVIEW)).toBe(CONNECTORS);
  });

  it("returns an organization page as readily as a project one", () => {
    expect(resolveReturnUrl(walk(CONNECTORS, ORG_MEMBERS), LIVE, OVERVIEW)).toBe(ORG_MEMBERS);
  });

  // You can leave a project, or lose your role in it, from inside Account —
  // and a rename moves its slug out from under the path that was recorded.
  it("walks past a project that is no longer listed", () => {
    expect(resolveReturnUrl(walk(CONNECTORS, "/acme/gone/api-keys"), LIVE, OVERVIEW)).toBe(
      CONNECTORS,
    );
  });

  it("walks past an organization you are no longer in", () => {
    expect(resolveReturnUrl(walk(CONNECTORS, "/globex/~/members"), LIVE, OVERVIEW)).toBe(
      CONNECTORS,
    );
    expect(resolveReturnUrl(walk(CONNECTORS, "/globex/web"), LIVE, OVERVIEW)).toBe(CONNECTORS);
  });

  it("falls back when no resource in the trail is still yours", () => {
    expect(resolveReturnUrl(walk("/acme/gone/api-keys"), LIVE, OVERVIEW)).toBe(OVERVIEW);
  });
});

describe("the resources the console is drawing", () => {
  it("lists every organization and every project by its root", () => {
    const live = liveResources(
      [{ slug: "acme" }, { slug: "globex" }],
      [
        { slug: "web", organizationSlug: "acme" },
        { slug: "web", organizationSlug: "globex" },
      ],
    );
    expect([...live].sort()).toEqual(["/acme", "/acme/web", "/globex", "/globex/web"]);
  });
});

describe("where a resource selector row points", () => {
  it("offers the overview for a resource never opened", () => {
    expect(resourceUrl(EMPTY_TRAIL, OTHER)).toBe("/acme/api");
    expect(resourceUrl(EMPTY_TRAIL, ORGANIZATION)).toBe("/acme");
  });

  it("returns you to the page you were reading in that resource", () => {
    const trail = walk(CONNECTORS, ORG_MEMBERS);
    expect(resourceUrl(trail, PROJECT)).toBe(CONNECTORS);
    expect(resourceUrl(trail, ORGANIZATION)).toBe(ORG_MEMBERS);
  });

  it("does not offer one resource's page for another", () => {
    expect(resourceUrl(walk(CONNECTORS), OTHER)).toBe("/acme/api");
    expect(resourceUrl(walk("/globex/web/connectors"), PROJECT)).toBe(OVERVIEW);
    expect(resourceUrl(walk(CONNECTORS), ORGANIZATION)).toBe("/acme");
  });
});

describe("which resource the chrome names", () => {
  it("names the one in the URL on every page that has one", () => {
    expect(standingResource(CONNECTORS, EMPTY_TRAIL, LIVE, OVERVIEW)).toEqual(PROJECT);
    expect(standingResource(ORG_MEMBERS, EMPTY_TRAIL, LIVE, OVERVIEW)).toEqual(ORGANIZATION);
    expect(standingResource("/acme", EMPTY_TRAIL, LIVE, OVERVIEW)).toEqual(ORGANIZATION);
  });

  // The path wins over the trail: it names where you ARE.
  it("names the one in the URL whatever the trail holds", () => {
    expect(standingResource(ORG_MEMBERS, walk(CONNECTORS), LIVE, OVERVIEW)).toEqual(
      ORGANIZATION,
    );
  });

  // The regression: Account has no resource in the URL, so the selector read
  // an empty segment and fell back to a placeholder while the rail's back
  // arrow still pointed at the project.
  it("keeps naming the project you stepped into Account from", () => {
    expect(standingResource(ACCOUNT, walk(CONNECTORS), LIVE, OVERVIEW)).toEqual(PROJECT);
  });

  it("names the organization when that is where you stepped in from", () => {
    expect(standingResource(ACCOUNT, walk(CONNECTORS, ORG_MEMBERS), LIVE, OVERVIEW)).toEqual(
      ORGANIZATION,
    );
  });

  it("agrees with the back arrow, which is the point of sharing the walk", () => {
    const trail = walk(CONNECTORS, "/acme/gone/api-keys");
    const standing = standingResource(ACCOUNT, trail, LIVE, OVERVIEW);
    expect(standing && resourceRoot(standing)).toBe(
      trailResource(resolveReturnUrl(trail, LIVE, OVERVIEW)),
    );
  });

  // A tab opened straight onto an account page has no trail to read.
  it("falls back to the first project, and to nothing without one", () => {
    expect(standingResource(ACCOUNT, EMPTY_TRAIL, LIVE, OVERVIEW)).toEqual(PROJECT);
    expect(standingResource(ACCOUNT, EMPTY_TRAIL, liveResources([], []), "/console")).toBeNull();
  });
});
