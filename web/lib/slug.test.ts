import { describe, expect, it } from "vitest";

import {
  RESERVED_ORGANIZATION_SLUGS,
  RESERVED_PROJECT_SLUGS,
  SLUG_MAX_LENGTH,
  isOrganizationPage,
  isOrganizationSlug,
  isSlug,
  organizationPath,
  projectPath,
  withLeadingSegments,
} from "./slug";

describe("isSlug", () => {
  it.each(["acme", "a", "web-app-2", "org-4k2j9x0q1z", "a".repeat(SLUG_MAX_LENGTH)])(
    "takes %s",
    (slug) => {
      expect(isSlug(slug)).toBe(true);
    },
  );

  // Each of these is something a path could otherwise misread: an id, a
  // capital the server never mints.
  it.each([
    "",
    "Acme",
    "-acme",
    "acme-",
    "web--app",
    "org_7bQx2mNv9BcK4dLp",
    "project_7bQx2mNv9BcK4dLp",
    "a.b",
    "a b",
    "a/b",
    "a".repeat(SLUG_MAX_LENGTH + 1),
  ])("refuses %j", (segment) => {
    expect(isSlug(segment)).toBe(false);
  });
});

describe("isOrganizationSlug", () => {
  it("takes a slug none of the console's own paths claims", () => {
    expect(isOrganizationSlug("acme")).toBe(true);
    expect(isOrganizationSlug("account-2")).toBe(true);
  });

  // The console's own first segments: an organization going by one would
  // shadow the page, or be shadowed by it.
  it.each(["account", "api", "auth", "console", "connect", "invite", "cli", "v1", "plans"])(
    "refuses %s",
    (word) => {
      expect(RESERVED_ORGANIZATION_SLUGS.has(word)).toBe(true);
      expect(isOrganizationSlug(word)).toBe(false);
    },
  );

  it("refuses what is no slug at all", () => {
    expect(isOrganizationSlug("org_7bQx2mNv9BcK4dLp")).toBe(false);
    expect(isOrganizationSlug("")).toBe(false);
  });

  it("reserves only words an organization could otherwise take", () => {
    for (const word of RESERVED_ORGANIZATION_SLUGS) {
      expect(isSlug(word), word).toBe(true);
    }
  });
});

describe("isOrganizationPage", () => {
  // An organization's pages sit beside its projects: a path's second segment
  // is a page exactly when it is a word no project goes by.
  it.each(["projects", "members", "audit-log", "settings", "billing"])("takes %s", (page) => {
    expect(isOrganizationPage(page)).toBe(true);
  });

  it("leaves every other slug to a project", () => {
    expect(isOrganizationPage("web")).toBe(false);
    expect(isOrganizationPage("settings-2")).toBe(false);
    expect(isOrganizationPage("account")).toBe(false);
  });

  it("reserves only words a project could otherwise take", () => {
    for (const word of RESERVED_PROJECT_SLUGS) {
      expect(isSlug(word), word).toBe(true);
    }
  });
});

describe("paths", () => {
  it("puts an organization's own pages directly under it", () => {
    expect(organizationPath("acme")).toBe("/acme");
    expect(organizationPath("acme", "/settings")).toBe("/acme/settings");
    expect(organizationPath("acme", "/members")).toBe("/acme/members");
  });

  it("puts a project directly under its organization", () => {
    expect(projectPath("acme", "web")).toBe("/acme/web");
    expect(projectPath("acme", "web", "/api-keys")).toBe("/acme/web/api-keys");
  });
});

describe("withLeadingSegments", () => {
  it("replaces an organization's id and keeps the rest", () => {
    expect(withLeadingSegments("/org_7bQx2mNv9BcK4dLp/billing", ["acme"])).toBe("/acme/billing");
    expect(withLeadingSegments("/Acme", ["acme"])).toBe("/acme");
  });

  it("replaces both segments of a project's path", () => {
    expect(
      withLeadingSegments("/acme/project_7bQx2mNv9BcK4dLp/audit-log", ["globex", "payments"]),
    ).toBe("/globex/payments/audit-log");
    expect(withLeadingSegments("/acme/project_7bQx2mNv9BcK4dLp", ["acme", "web"])).toBe(
      "/acme/web",
    );
  });

  it("keeps the query", () => {
    expect(
      withLeadingSegments("/org_1/project_1/connectors?connected=slack", ["acme", "web"]),
    ).toBe("/acme/web/connectors?connected=slack");
  });

  it("spells the bare path when there is nothing to keep", () => {
    expect(withLeadingSegments("/", ["acme"])).toBe("/acme");
    expect(withLeadingSegments("/", ["acme", "web"])).toBe("/acme/web");
  });
});
