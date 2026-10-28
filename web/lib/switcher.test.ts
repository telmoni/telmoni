import { describe, expect, it } from "vitest";

import { EMPTY_TRAIL, pushPath, type ConsoleTrail } from "./console-trail";
import { organizationRows, projectRows } from "./switcher";

const ACME = { organizationId: "org_acme", slug: "acme", name: "Acme" };
const BOLT = { organizationId: "org_bolt", slug: "bolt", name: "Bolt" };
const ZED = { organizationId: "org_zed", organizationSlug: "zed", organizationName: "Zed" };
const WEB = { id: "project_web", slug: "web", name: "Web" };
const API = { id: "project_api", slug: "api", name: "API" };

/** Walk the console the way the recorder does: one push per navigation. */
function walk(...paths: string[]): ConsoleTrail {
  return paths.reduce<ConsoleTrail>(pushPath, EMPTY_TRAIL);
}

describe("organizationRows", () => {
  it("lists the one stood in first, the others once each, then the ones reached through a project", () => {
    const rows = organizationRows({
      active: BOLT,
      organizations: [ACME, BOLT],
      elsewhere: [
        ZED,
        ZED,
        { organizationId: "org_acme", organizationSlug: "acme", organizationName: "Acme" },
      ],
      trail: EMPTY_TRAIL,
    });
    expect(rows.map((row) => [row.key, row.label, row.active])).toEqual([
      ["org_bolt", "Bolt", true],
      ["org_acme", "Acme", false],
      ["org_zed", "Zed", false],
    ]);
  });

  it("opens settings only where the person is a member", () => {
    const rows = organizationRows({
      active: ACME,
      organizations: [ACME],
      elsewhere: [ZED],
      trail: EMPTY_TRAIL,
    });
    expect(rows.map((row) => row.settingsHref)).toEqual(["/acme/settings", null]);
  });

  it("returns the one stood in to its last page, and opens the others at their overview", () => {
    const rows = organizationRows({
      active: ACME,
      organizations: [ACME, BOLT],
      elsewhere: [],
      trail: walk("/bolt/audit-log", "/acme/members"),
    });
    expect(rows.map((row) => row.href)).toEqual(["/acme/members", "/bolt"]);
  });

  it("marks nothing active outside an organization", () => {
    const rows = organizationRows({
      active: null,
      organizations: [ACME],
      elsewhere: [ZED],
      trail: EMPTY_TRAIL,
    });
    expect(rows.map((row) => row.active)).toEqual([false, false]);
  });
});

describe("projectRows", () => {
  it("lists the organization's projects, the one stood in marked, each returning to its last page", () => {
    const rows = projectRows({
      active: ACME,
      projects: [WEB, API],
      activeProjectId: "project_api",
      trail: walk("/acme/web/traces"),
    });
    expect(rows).toEqual([
      {
        key: "project_web",
        label: "Web",
        href: "/acme/web/traces",
        settingsHref: "/acme/web/settings",
        active: false,
      },
      {
        key: "project_api",
        label: "API",
        href: "/acme/api",
        settingsHref: "/acme/api/settings",
        active: true,
      },
    ]);
  });

  it("lists none outside an organization", () => {
    expect(
      projectRows({ active: null, projects: [WEB], activeProjectId: null, trail: EMPTY_TRAIL }),
    ).toEqual([]);
  });
});
