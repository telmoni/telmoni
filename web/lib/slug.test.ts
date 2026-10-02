import { describe, expect, it } from "vitest";
import {
  RESERVED_SLUGS,
  isValidSlug,
  organizationMatches,
  organizationSegment,
  projectMatches,
  projectSegment,
  slugify,
} from "./slug";

describe("slug utilities", () => {
  describe("RESERVED_SLUGS", () => {
    it("contains core reserved paths and sensitive keywords", () => {
      expect(RESERVED_SLUGS.has("api")).toBe(true);
      expect(RESERVED_SLUGS.has("auth")).toBe(true);
      expect(RESERVED_SLUGS.has("account")).toBe(true);
      expect(RESERVED_SLUGS.has("console")).toBe(true);
      expect(RESERVED_SLUGS.has("billing")).toBe(true);
      expect(RESERVED_SLUGS.has("settings")).toBe(true);
      expect(RESERVED_SLUGS.has("organization")).toBe(true);
      expect(RESERVED_SLUGS.has("projects")).toBe(true);
      expect(RESERVED_SLUGS.has("_next")).toBe(true);
    });
  });

  describe("isValidSlug", () => {
    it("accepts valid alphanumeric slugs with single hyphens", () => {
      expect(isValidSlug("acme")).toBe(true);
      expect(isValidSlug("acme-corp")).toBe(true);
      expect(isValidSlug("team-42-infra")).toBe(true);
      expect(isValidSlug("a-b")).toBe(true);
    });

    it("rejects reserved keywords", () => {
      expect(isValidSlug("api")).toBe(false);
      expect(isValidSlug("auth")).toBe(false);
      expect(isValidSlug("console")).toBe(false);
      expect(isValidSlug("billing")).toBe(false);
      expect(isValidSlug("CONSOLE")).toBe(false);
    });

    it("rejects malformed slugs", () => {
      expect(isValidSlug("")).toBe(false);
      expect(isValidSlug("a")).toBe(false); // too short
      expect(isValidSlug("-acme")).toBe(false); // leading hyphen
      expect(isValidSlug("acme-")).toBe(false); // trailing hyphen
      expect(isValidSlug("acme--corp")).toBe(false); // consecutive hyphens
      expect(isValidSlug("acme_corp")).toBe(false); // underscore
      expect(isValidSlug("acme.corp")).toBe(false); // dot
      expect(isValidSlug("acme corp")).toBe(false); // space
      expect(isValidSlug("a".repeat(49))).toBe(false); // over 48 chars
    });
  });

  describe("slugify", () => {
    it("converts spaces and special characters to hyphens", () => {
      expect(slugify("Acme Corporation, Inc.")).toBe("acme-corporation-inc");
      expect(slugify("Frontend & API Gateway")).toBe("frontend-api-gateway");
      expect(slugify("  My First Project  ")).toBe("my-first-project");
    });

    it("handles diacritics and unicode characters", () => {
      expect(slugify("Café & Résumé")).toBe("cafe-resume");
      expect(slugify("München Über")).toBe("munchen-uber");
    });

    it("handles strings with numbers and multiple hyphens", () => {
      expect(slugify("Team 42 - Core Infra")).toBe("team-42-core-infra");
      expect(slugify("---leading-and-trailing---")).toBe("leading-and-trailing");
      expect(slugify("foo----bar")).toBe("foo-bar");
    });

    it("truncates at 48 characters without a trailing hyphen", () => {
      const longName = "This is an extremely long organization name that exceeds 48 characters";
      const slug = slugify(longName);
      expect(slug.length).toBeLessThanOrEqual(48);
      expect(slug.endsWith("-")).toBe(false);
      expect(isValidSlug(slug)).toBe(true);
    });

    it("safely escapes reserved keywords with a qualifier", () => {
      expect(slugify("console", "org")).toBe("console-org");
      expect(slugify("api", "app")).toBe("api-app");
      expect(slugify("auth", "project")).toBe("auth-project");
      expect(isValidSlug(slugify("settings", "org"))).toBe(true);
    });

    it("returns empty string for inputs with fewer than 2 valid characters", () => {
      expect(slugify("")).toBe("");
      expect(slugify("?")).toBe("");
      expect(slugify("x")).toBe("");
    });
  });

  describe("organizationMatches", () => {
    const org = {
      organizationId: "org_01H123456789",
      slug: "acme-corp",
      name: "Acme Corp",
    };

    it("matches exact organizationId case-insensitively", () => {
      expect(organizationMatches(org, "org_01H123456789")).toBe(true);
      expect(organizationMatches(org, "ORG_01H123456789")).toBe(true);
    });

    it("matches explicit slug case-insensitively", () => {
      expect(organizationMatches(org, "acme-corp")).toBe(true);
      expect(organizationMatches(org, "ACME-CORP")).toBe(true);
    });

    it("matches slugified name when slug is absent", () => {
      const orgWithoutSlug = {
        organizationId: "org_01H123456789",
        name: "Acme Corp",
      };
      expect(organizationMatches(orgWithoutSlug, "acme-corp")).toBe(true);
      expect(organizationMatches(orgWithoutSlug, "ACME-CORP")).toBe(true);
    });

    it("rejects non-matching segments", () => {
      expect(organizationMatches(org, "other-corp")).toBe(false);
      expect(organizationMatches(org, "")).toBe(false);
    });
  });

  describe("projectMatches", () => {
    const project = {
      id: "project_01H123456789",
      slug: "web-dashboard",
      name: "Web Dashboard",
    };

    it("matches exact project ID case-insensitively", () => {
      expect(projectMatches(project, "project_01H123456789")).toBe(true);
      expect(projectMatches(project, "PROJECT_01H123456789")).toBe(true);
    });

    it("matches explicit slug case-insensitively", () => {
      expect(projectMatches(project, "web-dashboard")).toBe(true);
      expect(projectMatches(project, "WEB-DASHBOARD")).toBe(true);
    });

    it("matches slugified name when slug is absent", () => {
      const projectWithoutSlug = {
        id: "project_01H123456789",
        name: "Web Dashboard",
      };
      expect(projectMatches(projectWithoutSlug, "web-dashboard")).toBe(true);
    });

    it("rejects non-matching segments", () => {
      expect(projectMatches(project, "mobile-app")).toBe(false);
      expect(projectMatches(project, "")).toBe(false);
    });
  });

  describe("organizationSegment", () => {
    it("prefers explicit slug if available and valid", () => {
      expect(
        organizationSegment({
          organizationId: "org_123",
          slug: "my-org",
          name: "Company Name",
        }),
      ).toBe("my-org");
    });

    it("ignores invalid or reserved explicit slug", () => {
      expect(
        organizationSegment({
          organizationId: "org_123",
          slug: "console", // reserved!
          name: "Company Name",
        }),
      ).toBe("company-name");
    });

    it("falls back to slugified name if no explicit slug", () => {
      expect(
        organizationSegment({
          organizationId: "org_123",
          name: "Company Name",
        }),
      ).toBe("company-name");
    });

    it("falls back to organizationId if unnamed", () => {
      expect(
        organizationSegment({
          organizationId: "org_123",
          name: null,
        }),
      ).toBe("org_123");
    });
  });

  describe("projectSegment", () => {
    it("prefers explicit slug if available and valid", () => {
      expect(
        projectSegment({
          id: "project_123",
          slug: "my-project",
          name: "Project Name",
        }),
      ).toBe("my-project");
    });

    it("ignores reserved explicit slug and qualifies name", () => {
      expect(
        projectSegment({
          id: "project_123",
          slug: "billing", // reserved
          name: "Billing Service",
        }),
      ).toBe("billing-service");
    });

    it("falls back to slugified name if no explicit slug", () => {
      expect(
        projectSegment({
          id: "project_123",
          name: "Project Name",
        }),
      ).toBe("project-name");
    });

    it("falls back to id if unnamed or too short", () => {
      expect(
        projectSegment({
          id: "project_123",
          name: "",
        }),
      ).toBe("project_123");
    });
  });
});
