import { describe, expect, it } from "vitest";

import {
  ORGANIZATION_PAGES,
  RESERVED_ORGANIZATION_SLUGS,
  SLUG_MAX_LENGTH,
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

  // Each of these is something a path could otherwise misread: an id, the
  // organization's own-pages segment, a capital the server never mints.
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
    ORGANIZATION_PAGES,
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

describe("paths", () => {
  it("puts an organization's own pages under the segment no slug can be", () => {
    expect(organizationPath("acme")).toBe("/acme");
    expect(organizationPath("acme", "/settings")).toBe("/acme/~/settings");
    expect(organizationPath("acme", "/members")).toBe("/acme/~/members");
  });

  it("puts a project directly under its organization", () => {
    expect(projectPath("acme", "web")).toBe("/acme/web");
    expect(projectPath("acme", "web", "/api-keys")).toBe("/acme/web/api-keys");
  });

  // The reason for `~`: a project may take any slug, the name of one of its
  // organization's pages included, and the two paths stay apart.
  it("keeps a project called settings off the organization's settings", () => {
    expect(projectPath("acme", "settings")).not.toBe(organizationPath("acme", "/settings"));
    expect(projectPath("acme", "members", "/members")).toBe("/acme/members/members");
  });
});

describe("withLeadingSegments", () => {
  it("replaces an organization's id and keeps the rest", () => {
    expect(withLeadingSegments("/org_7bQx2mNv9BcK4dLp/~/billing", ["acme"])).toBe(
      "/acme/~/billing",
    );
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
