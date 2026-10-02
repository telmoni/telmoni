import { describe, expect, it } from "vitest";

import {
  EMPTY_TRAIL,
  TRAIL_LIMIT,
  isReturnablePath,
  parseTrail,
  pushPath,
  resolveReturnUrl,
  resourceUrl,
  standingResource,
  type ConsoleTrail,
} from "./console-trail";
import { rootSegment } from "./console-nav";

const PROJECT = "project_abc";
const OTHER = "project_def";
const PROJECTS = [PROJECT, OTHER];
const CONNECTORS = `/${PROJECT}/connectors`;
const KEYS = `/${PROJECT}/api-keys`;
const OVERVIEW = `/${PROJECT}`;
const ORG_MEMBERS = "/organization/members";

/** Walk the console the way the recorder does: one push per navigation. */
function walk(...paths: string[]): ConsoleTrail {
  return paths.reduce<ConsoleTrail>(pushPath, EMPTY_TRAIL);
}

describe("which paths the console may be sent back to", () => {
  it("takes any page under a project or the organization", () => {
    expect(isReturnablePath(CONNECTORS)).toBe(true);
    expect(isReturnablePath(OVERVIEW)).toBe(true);
    expect(isReturnablePath(ORG_MEMBERS)).toBe(true);
  });

  it("refuses the mode the back arrow leaves", () => {
    expect(isReturnablePath("/account")).toBe(false);
    expect(isReturnablePath("/account/settings")).toBe(false);
  });

  // `/console` resolves to the first project, so remembering it would send you
  // somewhere you never stood.
  it("refuses the console entry redirect", () => {
    expect(isReturnablePath("/console")).toBe(false);
  });

  it("refuses anything that could leave the origin", () => {
    expect(isReturnablePath("//evil.example.com")).toBe(false);
    expect(isReturnablePath("https://evil.example.com")).toBe(false);
    expect(isReturnablePath("/\\evil.example.com")).toBe(false);
    expect(isReturnablePath(`${PROJECT}/connectors`)).toBe(false);
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
      ...Array.from({ length: TRAIL_LIMIT + 1 }, (_, i) => `/project_${i}/api-keys`),
    );
    expect(trail).toHaveLength(TRAIL_LIMIT);
    expect(trail).not.toContain("/project_0/api-keys");
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
    const long = Array.from({ length: TRAIL_LIMIT + 3 }, (_, i) => `/project_${i}`);
    expect(parseTrail(JSON.stringify(long))).toHaveLength(TRAIL_LIMIT);
  });
});

describe("where the rail's back arrow points", () => {
  it("falls back when nothing was recorded", () => {
    expect(resolveReturnUrl(EMPTY_TRAIL, PROJECTS, OVERVIEW)).toBe(OVERVIEW);
  });

  it("returns the page you stepped in from", () => {
    expect(resolveReturnUrl(walk(CONNECTORS), PROJECTS, OVERVIEW)).toBe(CONNECTORS);
  });

  it("returns an organization page as readily as a project one", () => {
    expect(resolveReturnUrl(walk(CONNECTORS, ORG_MEMBERS), PROJECTS, OVERVIEW)).toBe(
      ORG_MEMBERS,
    );
  });

  // You can leave a project, or lose your role in it, from inside Account.
  it("walks past a project that is no longer listed", () => {
    expect(
      resolveReturnUrl(walk(CONNECTORS, "/project_gone/api-keys"), PROJECTS, OVERVIEW),
    ).toBe(CONNECTORS);
  });

  it("falls back when no resource in the trail is still yours", () => {
    expect(resolveReturnUrl(walk("/project_gone/api-keys"), PROJECTS, OVERVIEW)).toBe(
      OVERVIEW,
    );
  });
});

describe("where a resource selector row points", () => {
  it("offers the overview for a resource never opened", () => {
    expect(resourceUrl(EMPTY_TRAIL, OTHER)).toBe(`/${OTHER}`);
    expect(resourceUrl(EMPTY_TRAIL, "organization")).toBe("/organization");
  });

  it("returns you to the page you were reading in that resource", () => {
    const trail = walk(CONNECTORS, ORG_MEMBERS);
    expect(resourceUrl(trail, PROJECT)).toBe(CONNECTORS);
    expect(resourceUrl(trail, "organization")).toBe(ORG_MEMBERS);
  });

  it("does not offer one resource's page for another", () => {
    expect(resourceUrl(walk(CONNECTORS), OTHER)).toBe(`/${OTHER}`);
  });
});

describe("which resource the chrome names", () => {
  it("names the one in the URL on every page that has one", () => {
    expect(standingResource(PROJECT, EMPTY_TRAIL, PROJECTS)).toBe(PROJECT);
    expect(standingResource("organization", EMPTY_TRAIL, PROJECTS)).toBe(
      "organization",
    );
  });

  // The regression: Account has no resource in the URL, so the selector read
  // an empty segment and fell back to a placeholder while the rail's back
  // arrow still pointed at the project.
  it("keeps naming the project you stepped into Account from", () => {
    expect(standingResource("account", walk(CONNECTORS), PROJECTS)).toBe(PROJECT);
  });

  it("names the organization when that is where you stepped in from", () => {
    expect(standingResource("account", walk(CONNECTORS, ORG_MEMBERS), PROJECTS)).toBe(
      "organization",
    );
  });

  it("agrees with the back arrow, which is the point of sharing the walk", () => {
    const trail = walk(CONNECTORS, "/project_gone/api-keys");
    expect(standingResource("account", trail, PROJECTS)).toBe(
      rootSegment(resolveReturnUrl(trail, PROJECTS, OVERVIEW)),
    );
  });

  // A tab opened straight onto an account page has no trail to read.
  it("falls back to the first project, and to nothing without one", () => {
    expect(standingResource("account", EMPTY_TRAIL, PROJECTS)).toBe(PROJECT);
    expect(standingResource("account", EMPTY_TRAIL, [])).toBe("");
  });
});

describe("hierarchical slug trails", () => {
  it("tracks organization and project pages by slug resource", () => {
    const trail = walk("/acme/web-app", "/acme/projects");
    expect(trail).toEqual(["/acme/projects", "/acme/web-app"]);
  });

  it("replaces trail entry when navigating within the same project by slug", () => {
    const trail = walk("/acme/web-app", "/acme/web-app/connectors");
    expect(trail).toEqual(["/acme/web-app/connectors"]);
  });

  it("replaces trail entry when navigating within the same organization by slug", () => {
    const trail = walk("/acme/projects", "/acme/billing");
    expect(trail).toEqual(["/acme/billing"]);
  });
});
