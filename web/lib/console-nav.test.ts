import { describe, expect, it } from "vitest";

import { FileText, Settings, Users } from "lucide-react";

import { buildConsoleNav, isAccountPath, rootSegment, withExtraItems } from "./console-nav";
import { Flag } from "./types/enums";

describe("buildConsoleNav", () => {
  it("gives every destination a project-scoped address", () => {
    for (const group of buildConsoleNav("/prj-123/api-keys", "prj-123")) {
      for (const item of group.items) {
        expect(item.url.startsWith("/prj-123")).toBe(true);
      }
    }
  });

  it("returns exactly one run, whichever rail the address is on", () => {
    expect(buildConsoleNav("/prj-123/api-keys", "prj-123").map((g) => g.title)).toEqual(["Project"]);
    expect(buildConsoleNav("/organization/projects", "organization").map((g) => g.title)).toEqual([
      "Organization",
    ]);
    expect(buildConsoleNav("/account/settings", "account").map((g) => g.title)).toEqual([
      "Account",
    ]);
  });

  it("builds each run against its own segment", () => {
    const [account] = buildConsoleNav("/account/settings", "account");
    expect(account?.items.map((i) => i.url)).toEqual([
      "/account/settings",
      "/account/notifications",
      "/account/privacy",
    ]);
    const [organization] = buildConsoleNav("/organization", "organization");
    expect(organization?.items.every((i) => i.url.startsWith("/organization"))).toBe(true);
  });

  it("keeps the person's rail out of the organization's, and the reverse", () => {
    const account = buildConsoleNav("/account/notifications", "account");
    expect(account.map((g) => g.title)).toEqual(["Account"]);
    expect(account.flatMap((g) => g.items.map((i) => i.url))).not.toContain(
      "/organization/settings",
    );

    const organization = buildConsoleNav("/organization/settings", "organization");
    expect(organization.flatMap((g) => g.items.map((i) => i.url))).not.toContain(
      "/account/settings",
    );
  });

  it("reads an organization path under a project segment as the project's", () => {
    expect(
      buildConsoleNav("/prj-123/organization", "prj-123").map((g) => g.title),
    ).toEqual(["Project"]);
  });

  it("draws the organization's rail on the pairing page", () => {
  });

  it("leads the project run with Overview, then the organization's objects, and Settings last", () => {
    const [project] = buildConsoleNav("/prj-123/api-keys", "prj-123");
    expect(project?.items.map((i) => i.url)).toEqual([
      "/prj-123",
      "/prj-123/api-keys",
      "/prj-123/connectors",
      "/prj-123/members",
      "/prj-123/audit-log",
      "/prj-123/settings",
    ]);
  });

  // ⚠ **The rail takes no role, and draws every organization row.** It used
  // to drop Members, Audit log and Settings for a member, so a member
  // standing in somebody else's organization saw two rows and no hint of what
  // an organization has or whom to ask. The page behind a row refuses now,
  // naming what the role lacks and the owner to ask (`RoleRestricted`,
  // `AccessDenied`), which a missing row could never say.
  it("draws every organization destination, whoever is looking", () => {
    const org = buildConsoleNav("/organization", "organization").find(
      (g) => g.title === "Organization",
    );
    expect(org?.items.map((i) => i.url)).toEqual([
      "/organization",
      "/organization/projects",
      "/organization/members",
      "/organization/audit-log",
      "/organization/settings",
    ]);

    const orgMode = buildConsoleNav("/organization/settings", "organization").find(
      (g) => g.title === "Organization",
    );
    expect(orgMode?.items.find((i) => i.title === "Settings")?.isActive).toBe(true);
  });

  it("offers no row for a page this build does not have", () => {
    const urls = buildConsoleNav("/prj-123/api-keys", "prj-123").flatMap((g) =>
      g.items.map((i) => i.url),
    );
    for (const gone of [
      "/prj-123/projects",
      "/prj-123/projects",
      "/prj-123/project",
      "/prj-123/invites",
      "/prj-123/overview",
      "/prj-123/notifications",
    ]) {
      expect(urls).not.toContain(gone);
    }
  });

  it("offers no Notifications row at either level", () => {
    for (const [pathname, segment] of [
      ["/prj-123/api-keys", "prj-123"],
      ["/organization", "organization"],
    ] as const) {
      const items = buildConsoleNav(pathname, segment).flatMap((g) => g.items);
      expect(items.map((i) => i.url)).not.toContain(`/${segment}/notifications`);
      expect(items.map((i) => i.title)).not.toContain("Notifications");
    }
  });

  // The title is the heading drawn over the run, so the two have to stay the
  // same string: a group renamed for the switcher and not for the rail would
  // head the Organization list with the word Project. Project and
  // Organization carry the same row names, and Account is headed the same way
  // so the three rails read as one kind of list.
  it("heads every run with the level's own name", () => {
    for (const [pathname, segment, title] of [
      ["/prj-123/api-keys", "prj-123", "Project"],
      ["/organization/projects", "organization", "Organization"],
      ["/account/settings", "account", "Account"],
    ] as const) {
      const [group] = buildConsoleNav(pathname, segment);
      expect(group?.title).toBe(title);
    }
  });

  it("marks exactly the current destination active, by subtree", () => {
    const active = (pathname: string) =>
      buildConsoleNav(pathname, "prj-123")
        .flatMap((g) => g.items)
        .filter((i) => i.isActive)
        .map((i) => i.url);

    expect(active("/prj-123/api-keys")).toEqual(["/prj-123/api-keys"]);
    expect(active("/prj-123/api-keys/abc")).toEqual(["/prj-123/api-keys"]);
    expect(active("/prj-123/connectors")).toEqual(["/prj-123/connectors"]);

    // A sibling that merely BEGINS with a row's path is not under it. No such
    // route exists yet, which is the point: the day one is added, the rail
    // must not light two rows because one name is a prefix of another.
    expect(active("/prj-123/api-keys-archive"), "a sibling of /api-keys").toEqual([]);
    expect(active("/prj-123/settings-v2"), "a sibling of /settings").toEqual([]);
  });

  // The rows a role cannot open stay on both rails: the audit log, which a
  // project member and an organization admin are refused, and the
  // organization's settings, which a member is. The page says so.
  it("draws the rows a role cannot open, and leaves the refusal to the page", () => {
    const project = buildConsoleNav("/prj-123/api-keys", "prj-123").flatMap((g) =>
      g.items.map((i) => i.url),
    );
    expect(project).toEqual([
      "/prj-123",
      "/prj-123/api-keys",
      "/prj-123/connectors",
      "/prj-123/members",
      "/prj-123/audit-log",
      "/prj-123/settings",
    ]);

    const organization = buildConsoleNav("/organization/settings", "organization").flatMap(
      (g) => g.items.map((i) => i.url),
    );
    expect(organization).toContain("/organization/audit-log");
    expect(organization).toContain("/organization/settings");
  });

  it("drops a row whose every flag is off, and keeps one with a flag still on", () => {
    const off = buildConsoleNav("/prj-123/api-keys", "prj-123", {
      [Flag.ApiTokens]: false,
      [Flag.PublicApi]: false,
    });
    expect(off.flatMap((g) => g.items).map((i) => i.url)).toContain("/prj-123/api-keys");
    expect(off.flatMap((g) => g.items).map((i) => i.url)).toContain("/prj-123/connectors");
  });
});

describe("the retired rows", () => {
  it("draws no observability row", () => {
    const urls = [
      ...buildConsoleNav("/prj-123/api-keys", "prj-123"),
      ...buildConsoleNav("/organization/settings", "organization"),
    ].flatMap((g) => g.items.map((i) => i.url));
    for (const gone of [
      "/prj-123/services",
      "/prj-123/slos",
      "/prj-123/alerts",
      "/prj-123/metrics",
      "/prj-123/logs",
      "/prj-123/traces",
    ]) {
      expect(urls).not.toContain(gone);
    }
  });

  it("draws no Heartbeats row", () => {
    const urls = [
      ...buildConsoleNav("/prj-123/api-keys", "prj-123"),
      ...buildConsoleNav("/organization/settings", "organization"),
    ].flatMap((g) => g.items.map((i) => i.url));
    expect(urls).not.toContain("/prj-123/heartbeats");
    expect(urls.some((u) => u.includes("heartbeat"))).toBe(false);
  });
});

describe("the whole destination set", () => {
  it("is exactly these destinations, in this order", () => {
    expect(
      [
        ...buildConsoleNav("/prj-123/api-keys", "prj-123"),
        ...buildConsoleNav("/organization/settings", "organization"),
        ...buildConsoleNav("/account/settings", "account"),
      ].flatMap((g) => g.items.map((i) => i.url)),
    ).toEqual([
      "/prj-123",
      "/prj-123/api-keys",
      "/prj-123/connectors",
      "/prj-123/members",
      "/prj-123/audit-log",
      "/prj-123/settings",
      "/organization",
      "/organization/projects",
      "/organization/members",
      "/organization/audit-log",
      "/organization/settings",
      "/account/settings",
      "/account/notifications",
      "/account/privacy",
    ]);
  });
});

describe("rootSegment", () => {
  it("reads the first segment of a console path", () => {
    expect(rootSegment("/prj-123/api-keys")).toBe("prj-123");
    expect(rootSegment("/prj-123")).toBe("prj-123");
    expect(rootSegment("/organization/members")).toBe("organization");
    expect(rootSegment("/account/settings")).toBe("account");
  });

  it("is empty for the root and for nothing", () => {
    expect(rootSegment("/")).toBe("");
    expect(rootSegment("")).toBe("");
  });

  // The inverse of `build()`: every URL the rail writes reads back to the
  // root it was written from.
  it("inverts every url the rail builds", () => {
    for (const [pathname, root] of [
      ["/prj-123/api-keys", "prj-123"],
      ["/organization/settings", "organization"],
      ["/account/settings", "account"],
    ] as const) {
      for (const group of buildConsoleNav(pathname, root)) {
        for (const item of group.items) {
          expect(rootSegment(item.url)).toBe(root);
        }
      }
    }
  });
});

describe("isAccountPath", () => {
  it("covers the account root and everything under it", () => {
    expect(isAccountPath("/account")).toBe(true);
    expect(isAccountPath("/account/settings")).toBe(true);
    expect(isAccountPath("/account/")).toBe(true);
  });

  // A sibling whose name begins with the word is a different place.
  it("does not claim a path that merely starts with the word", () => {
    expect(isAccountPath("/accounting")).toBe(false);
    expect(isAccountPath("/prj-123/account")).toBe(false);
    expect(isAccountPath("/")).toBe(false);
  });
});

describe("withExtraItems", () => {
  const core = [
    {
      title: "Organization",
      segment: "organization",
      items: [
        { title: "Members", path: "/members", icon: Users },
        { title: "Settings", path: "/settings", icon: Settings },
      ],
    },
  ];
  const titles = (groups: ReturnType<typeof withExtraItems>) =>
    groups[0]?.items.map((i) => i.title);

  it("leaves the core's rows as they are when nothing is added", () => {
    expect(titles(withExtraItems(core, []))).toEqual(["Members", "Settings"]);
  });

  it("puts a row above the one it names, and last when it names none", () => {
    const reports = { group: "Organization" as const, title: "Reports", path: "/reports", icon: FileText };
    expect(titles(withExtraItems(core, [{ ...reports, before: "Settings" }]))).toEqual([
      "Members",
      "Reports",
      "Settings",
    ]);
    expect(titles(withExtraItems(core, [reports]))).toEqual(["Members", "Settings", "Reports"]);
    expect(titles(withExtraItems(core, [{ ...reports, before: "Nowhere" }]))).toEqual([
      "Members",
      "Settings",
      "Reports",
    ]);
  });

  it("adds a row only to the group it names", () => {
    const row = { group: "Project" as const, title: "Reports", path: "/reports", icon: FileText };
    expect(titles(withExtraItems(core, [row]))).toEqual(["Members", "Settings"]);
  });
});
