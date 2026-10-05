import { describe, expect, it } from "vitest";

import { FileText, Settings, Users } from "lucide-react";

import {
  buildConsoleNav,
  consolePlace,
  isAccountPath,
  movedPath,
  organizationToRemember,
  projectAt,
  rootSegment,
  staleShellOrganization,
  withExtraItems,
} from "./console-nav";
import { Flag } from "./types/enums";

describe("buildConsoleNav", () => {
  it("gives every destination a project-scoped address", () => {
    for (const group of buildConsoleNav("/acme/web/api-keys")) {
      for (const item of group.items) {
        expect(item.url.startsWith("/acme/web")).toBe(true);
      }
    }
  });

  it("returns exactly one run, whichever rail the address is on", () => {
    expect(buildConsoleNav("/acme/web/api-keys").map((g) => g.title)).toEqual(["Project"]);
    expect(buildConsoleNav("/acme/projects").map((g) => g.title)).toEqual(["Organization"]);
    expect(buildConsoleNav("/acme").map((g) => g.title)).toEqual(["Organization"]);
    expect(buildConsoleNav("/account/settings").map((g) => g.title)).toEqual(["Account"]);
  });

  it("builds each run against its own segment", () => {
    const [account] = buildConsoleNav("/account/settings");
    expect(account?.items.map((i) => i.url)).toEqual([
      "/account/settings",
      "/account/notifications",
      "/account/privacy",
    ]);
    const [organization] = buildConsoleNav("/acme");
    for (const item of organization?.items ?? []) {
      expect(consolePlace(item.url), item.url).toEqual({
        kind: "organization",
        organization: "acme",
      });
    }
  });

  it("keeps the person's rail out of the organization's, and the reverse", () => {
    const account = buildConsoleNav("/account/notifications");
    expect(account.map((g) => g.title)).toEqual(["Account"]);
    expect(account.flatMap((g) => g.items.map((i) => i.url))).not.toContain("/acme/settings");

    const organization = buildConsoleNav("/acme/settings");
    expect(organization.flatMap((g) => g.items.map((i) => i.url))).not.toContain(
      "/account/settings",
    );
  });

  // ⚠ **The bug this scheme exists to end.** `/acme/members` was read as a
  // page of a PROJECT called `acme`, so every organization page drew the
  // project rail, with links into a project that did not exist. An
  // organization's own pages go by words no project may
  // (`RESERVED_PROJECT_SLUGS`); any other second segment is a project.
  it("draws the organization's rail on every one of its own pages", () => {
    for (const page of ["projects", "members", "audit-log", "settings", "billing"]) {
      const [group] = buildConsoleNav(`/acme/${page}`);
      expect(group?.title, page).toBe("Organization");
      expect(group?.items[0]?.url, page).toBe("/acme");
    }
  });

  // A project called Settings goes by `settings-2`, a page's word being taken.
  it("reads a project named like an organization page as the project", () => {
    for (const pathname of ["/acme/settings-2", "/acme/members-2/members", "/acme/projects-2"]) {
      expect(buildConsoleNav(pathname).map((g) => g.title), pathname).toEqual(["Project"]);
    }
    expect(buildConsoleNav("/acme/settings-2")[0]?.items.map((i) => i.url)).toContain(
      "/acme/settings-2/settings",
    );
  });

  it("draws no rail off the console's own paths", () => {
    for (const pathname of ["/", "/console", "/api/health", "/org_7bQx2mNv9BcK4dLp/web"]) {
      expect(buildConsoleNav(pathname), pathname).toEqual([]);
    }
  });

  it("draws the organization's rail on the pairing page", () => {
  });

  it("leads the project run with Overview, then the organization's objects, and Settings last", () => {
    const [project] = buildConsoleNav("/acme/web/api-keys");
    expect(project?.items.map((i) => i.url)).toEqual([
      "/acme/web",
      "/acme/web/api-keys",
      "/acme/web/connectors",
      "/acme/web/members",
      "/acme/web/audit-log",
      "/acme/web/settings",
    ]);
  });

  // ⚠ **The rail takes no role, and draws every organization row.** It used
  // to drop Members, Audit log and Settings for a member, so a member
  // standing in somebody else's organization saw two rows and no hint of what
  // an organization has or whom to ask. The page behind a row refuses now,
  // naming what the role lacks and the owner to ask (`RoleRestricted`,
  // `AccessDenied`), which a missing row could never say.
  it("draws every organization destination, whoever is looking", () => {
    const org = buildConsoleNav("/acme").find((g) => g.title === "Organization");
    expect(org?.items.map((i) => i.url)).toEqual([
      "/acme",
      "/acme/projects",
      "/acme/members",
      "/acme/audit-log",
      "/acme/settings",
    ]);

    const orgMode = buildConsoleNav("/acme/settings").find((g) => g.title === "Organization");
    expect(orgMode?.items.find((i) => i.title === "Settings")?.isActive).toBe(true);
    expect(orgMode?.items.find((i) => i.title === "Overview")?.isActive).toBe(false);
  });

  it("offers no row for a page this build does not have", () => {
    const urls = buildConsoleNav("/acme/web/api-keys").flatMap((g) =>
      g.items.map((i) => i.url),
    );
    for (const gone of [
      "/acme/web/projects",
      "/acme/web/project",
      "/acme/web/invites",
      "/acme/web/overview",
      "/acme/web/notifications",
    ]) {
      expect(urls).not.toContain(gone);
    }
  });

  it("offers no Notifications row at either level", () => {
    for (const pathname of ["/acme/web/api-keys", "/acme"]) {
      const items = buildConsoleNav(pathname).flatMap((g) => g.items);
      expect(items.some((i) => i.url.endsWith("/notifications"))).toBe(false);
      expect(items.map((i) => i.title)).not.toContain("Notifications");
    }
  });

  // The title is the heading drawn over the run, so the two have to stay the
  // same string: a group renamed for the switcher and not for the rail would
  // head the Organization list with the word Project. Project and
  // Organization carry the same row names, and Account is headed the same way
  // so the three rails read as one kind of list.
  it("heads every run with the level's own name", () => {
    for (const [pathname, title] of [
      ["/acme/web/api-keys", "Project"],
      ["/acme/projects", "Organization"],
      ["/account/settings", "Account"],
    ] as const) {
      const [group] = buildConsoleNav(pathname);
      expect(group?.title).toBe(title);
    }
  });

  it("marks exactly the current destination active, by subtree", () => {
    const active = (pathname: string) =>
      buildConsoleNav(pathname)
        .flatMap((g) => g.items)
        .filter((i) => i.isActive)
        .map((i) => i.url);

    expect(active("/acme/web")).toEqual(["/acme/web"]);
    expect(active("/acme/web/api-keys")).toEqual(["/acme/web/api-keys"]);
    expect(active("/acme/web/api-keys/abc")).toEqual(["/acme/web/api-keys"]);
    expect(active("/acme/web/connectors")).toEqual(["/acme/web/connectors"]);
    expect(active("/acme")).toEqual(["/acme"]);
    expect(active("/acme/members")).toEqual(["/acme/members"]);

    // A sibling that merely BEGINS with a row's path is not under it. No such
    // route exists yet, which is the point: the day one is added, the rail
    // must not light two rows because one name is a prefix of another.
    expect(active("/acme/web/api-keys-archive"), "a sibling of /api-keys").toEqual([]);
    expect(active("/acme/web/settings-v2"), "a sibling of /settings").toEqual([]);
  });

  // The rows a role cannot open stay on both rails: the audit log, which a
  // project member and an organization admin are refused, and the
  // organization's settings, which a member is. The page says so.
  it("draws the rows a role cannot open, and leaves the refusal to the page", () => {
    const project = buildConsoleNav("/acme/web/api-keys").flatMap((g) =>
      g.items.map((i) => i.url),
    );
    expect(project).toEqual([
      "/acme/web",
      "/acme/web/api-keys",
      "/acme/web/connectors",
      "/acme/web/members",
      "/acme/web/audit-log",
      "/acme/web/settings",
    ]);

    const organization = buildConsoleNav("/acme/settings").flatMap((g) =>
      g.items.map((i) => i.url),
    );
    expect(organization).toContain("/acme/audit-log");
    expect(organization).toContain("/acme/settings");
  });

  it("drops a row whose every flag is off, and keeps one with a flag still on", () => {
    const off = buildConsoleNav("/acme/web/api-keys", {
      [Flag.ApiTokens]: false,
      [Flag.PublicApi]: false,
    });
    expect(off.flatMap((g) => g.items).map((i) => i.url)).toContain("/acme/web/api-keys");
    expect(off.flatMap((g) => g.items).map((i) => i.url)).toContain("/acme/web/connectors");
  });
});

describe("the retired rows", () => {
  it("draws no observability row", () => {
    const urls = [
      ...buildConsoleNav("/acme/web/api-keys"),
      ...buildConsoleNav("/acme/settings"),
    ].flatMap((g) => g.items.map((i) => i.url));
    for (const gone of [
      "/acme/web/services",
      "/acme/web/slos",
      "/acme/web/alerts",
      "/acme/web/metrics",
      "/acme/web/logs",
      "/acme/web/traces",
    ]) {
      expect(urls).not.toContain(gone);
    }
  });

  it("draws no Heartbeats row", () => {
    const urls = [
      ...buildConsoleNav("/acme/web/api-keys"),
      ...buildConsoleNav("/acme/settings"),
    ].flatMap((g) => g.items.map((i) => i.url));
    expect(urls).not.toContain("/acme/web/heartbeats");
    expect(urls.some((u) => u.includes("heartbeat"))).toBe(false);
  });
});

describe("the whole destination set", () => {
  it("is exactly these destinations, in this order", () => {
    expect(
      [
        ...buildConsoleNav("/acme/web/api-keys"),
        ...buildConsoleNav("/acme/settings"),
        ...buildConsoleNav("/account/settings"),
      ].flatMap((g) => g.items.map((i) => i.url)),
    ).toEqual([
      "/acme/web",
      "/acme/web/api-keys",
      "/acme/web/connectors",
      "/acme/web/members",
      "/acme/web/audit-log",
      "/acme/web/settings",
      "/acme",
      "/acme/projects",
      "/acme/members",
      "/acme/audit-log",
      "/acme/settings",
      "/account/settings",
      "/account/notifications",
      "/account/privacy",
    ]);
  });
});

describe("rootSegment", () => {
  it("reads the first segment of a console path", () => {
    expect(rootSegment("/acme/web/api-keys")).toBe("acme");
    expect(rootSegment("/acme")).toBe("acme");
    expect(rootSegment("/acme/members")).toBe("acme");
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
      ["/acme/web/api-keys", "acme"],
      ["/acme/settings", "acme"],
      ["/account/settings", "account"],
    ] as const) {
      for (const group of buildConsoleNav(pathname)) {
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
    expect(isAccountPath("/acme/account")).toBe(false);
    expect(isAccountPath("/")).toBe(false);
  });
});

describe("withExtraItems", () => {
  const core = [
    {
      title: "Organization",
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

describe("consolePlace", () => {
  it("reads an organization off its overview and its own pages", () => {
    const organization = { kind: "organization", organization: "acme" };
    for (const pathname of ["/acme", "/acme/", "/acme/settings", "/acme/billing", "/acme/members/x"]) {
      expect(consolePlace(pathname), pathname).toEqual(organization);
    }
  });

  it("reads any other second segment as a project", () => {
    expect(consolePlace("/acme/web-app")).toEqual({
      kind: "project",
      organization: "acme",
      project: "web-app",
    });
    expect(consolePlace("/acme/web-app/connectors")).toEqual({
      kind: "project",
      organization: "acme",
      project: "web-app",
    });
    // A word the console keeps at the top is no page of an organization's.
    for (const named of ["settings-2", "members-2", "account", "console"]) {
      expect(consolePlace(`/acme/${named}`), named).toEqual({
        kind: "project",
        organization: "acme",
        project: named,
      });
    }
  });

  it("reads Account as a place of its own, and nothing off the console's paths", () => {
    expect(consolePlace("/account/settings")).toEqual({ kind: "account" });
    expect(consolePlace("/account")).toEqual({ kind: "account" });
    for (const pathname of [
      "/",
      "",
      "/console",
      "/api/events",
      "/auth/login",
      "/invite/abc",
      // An id is redirected to its slug before anything is drawn for it.
      "/org_7bQx2mNv9BcK4dLp",
      "/org_7bQx2mNv9BcK4dLp/project_7bQx2mNv9BcK4dLp",
      "/Acme/web",
    ]) {
      expect(consolePlace(pathname), pathname).toBeNull();
    }
  });
});

describe("projectAt", () => {
  const projects = [
    { id: "project_1", slug: "web" },
    { id: "project_2", slug: "settings-2" },
  ];

  it("finds the project the path names in the organization's listing", () => {
    expect(projectAt("/acme/web", "acme", projects)?.id).toBe("project_1");
    expect(projectAt("/acme/web/api-keys", "acme", projects)?.id).toBe("project_1");
    expect(projectAt("/acme/settings-2/settings", "acme", projects)?.id).toBe("project_2");
  });

  it("finds none off a project's pages", () => {
    expect(projectAt("/acme", "acme", projects)).toBeNull();
    expect(projectAt("/acme/settings", "acme", projects)).toBeNull();
    expect(projectAt("/account/settings", "acme", projects)).toBeNull();
    expect(projectAt("/acme/gone", "acme", projects)).toBeNull();
  });

  // Two organizations may each hold a project called `web`. While the store
  // still lists the one just left, the path's project is not in it.
  it("finds none while the listing is another organization's", () => {
    expect(projectAt("/globex/web", "acme", projects)).toBeNull();
    expect(projectAt("/acme/web", null, projects)).toBeNull();
  });
});

describe("staleShellOrganization", () => {
  const organizations = [{ slug: "acme" }, { slug: "globex" }];

  it("names the organization the path moved into", () => {
    expect(staleShellOrganization("/globex/web", "acme", organizations)).toBe("globex");
    expect(staleShellOrganization("/globex/members", "acme", organizations)).toBe("globex");
    expect(staleShellOrganization("/globex", null, organizations)).toBe("globex");
  });

  it("is quiet while the shell and the path agree", () => {
    expect(staleShellOrganization("/acme/web", "acme", organizations)).toBeNull();
    expect(staleShellOrganization("/acme", "acme", organizations)).toBeNull();
  });

  it("is quiet on a path that names no organization", () => {
    expect(staleShellOrganization("/account/settings", "acme", organizations)).toBeNull();
    expect(staleShellOrganization("/console", "acme", organizations)).toBeNull();
  });

  // Auth answers such a path with one of the person's own organizations, so
  // asking again would get the same answer, again and again.
  it("is quiet for an organization the person is not in", () => {
    expect(staleShellOrganization("/initech/web", "acme", organizations)).toBeNull();
    expect(staleShellOrganization("/initech/web", "acme", [])).toBeNull();
  });
});

describe("movedPath", () => {
  it("follows an organization's slug on every page under it", () => {
    const moved = { from: "org-4k2j9x0q1z", to: "acme" };
    expect(movedPath("/org-4k2j9x0q1z", moved)).toBe("/acme");
    expect(movedPath("/org-4k2j9x0q1z/settings", moved)).toBe("/acme/settings");
    expect(movedPath("/org-4k2j9x0q1z/web/api-keys", moved)).toBe("/acme/web/api-keys");
  });

  it("follows a project's slug in the organization that holds it", () => {
    const moved = { organization: "acme", from: "default-project", to: "web" };
    expect(movedPath("/acme/default-project", moved)).toBe("/acme/web");
    expect(movedPath("/acme/default-project/members", moved)).toBe("/acme/web/members");
  });

  it("leaves a path that is not spelled with the slug", () => {
    expect(movedPath("/globex/web", { from: "acme", to: "acme-robotics" })).toBeNull();
    expect(movedPath("/account/settings", { from: "account", to: "acme" })).toBeNull();
    expect(movedPath("/console", { from: "console", to: "acme" })).toBeNull();
  });

  // ⚠ Two organizations may each hold a project called `web`, and an
  // organization may go by the slug a project just left.
  it("never follows a project's move in another organization, or onto an organization's pages", () => {
    const moved = { organization: "acme", from: "web", to: "site" };
    expect(movedPath("/globex/web", moved)).toBeNull();
    expect(movedPath("/web/settings", moved)).toBeNull();
    expect(movedPath("/acme/settings", moved)).toBeNull();
    expect(movedPath("/acme", moved)).toBeNull();
    expect(movedPath("/acme/api/web", moved)).toBeNull();
  });
});

describe("organizationToRemember", () => {
  const organizations = [
    { organizationId: "org_acme", slug: "acme" },
    { organizationId: "org_globex", slug: "globex" },
  ];

  // By id: the cookie outlives the page, and a URL change moves a slug.
  it("remembers the organization of the page on screen, by its id", () => {
    expect(organizationToRemember("/globex/web", organizations, "org_acme")).toBe("org_globex");
    expect(organizationToRemember("/globex/members", organizations, null)).toBe("org_globex");
    expect(organizationToRemember("/acme", organizations, "org_acme")).toBe("org_acme");
  });

  // Account names none, so the cookie keeps following the organization this
  // tab stands in — and not the one another tab moved it to since.
  it("remembers the one the console stands in on a path that names none", () => {
    expect(organizationToRemember("/account/settings", organizations, "org_acme")).toBe("org_acme");
    expect(organizationToRemember("/account/settings", organizations, null)).toBeNull();
  });

  // A path that answers 404 opened nothing.
  it("remembers nothing for an organization the person is not in", () => {
    expect(organizationToRemember("/initech/web", organizations, "org_acme")).toBeNull();
    expect(organizationToRemember("/acme/web", [], "org_acme")).toBeNull();
  });
});
