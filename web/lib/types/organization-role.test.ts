import { describe, expect, it } from "vitest";

import {
  OrganizationRole,
  asOrganizationRole,
  canCreateProjects,
  canDeleteProjects,
  canManageOrganizationMembers,
  canManageOrganizationSettings,
  canViewOrganizationMembers,
  canViewRolledUpAudit,
} from "./organization-role";

describe("OrganizationRole", () => {
  it("parses valid roles and rejects unknown", () => {
    expect(asOrganizationRole("owner")).toBe(OrganizationRole.Owner);
    expect(asOrganizationRole("admin")).toBe(OrganizationRole.Admin);
    expect(asOrganizationRole("member")).toBe(OrganizationRole.Member);
    expect(asOrganizationRole("OWNER")).toBe(OrganizationRole.Owner);
    expect(asOrganizationRole("viewer")).toBeNull();
    expect(asOrganizationRole("editor")).toBeNull();
    expect(asOrganizationRole(123)).toBeNull();
  });

  describe("permissions per role specification", () => {
    it("grants Owner full permissions", () => {
      expect(canCreateProjects(OrganizationRole.Owner)).toBe(true);
      expect(canDeleteProjects(OrganizationRole.Owner)).toBe(true);
      expect(canManageOrganizationSettings(OrganizationRole.Owner)).toBe(true);
      expect(canViewRolledUpAudit(OrganizationRole.Owner)).toBe(true);
      expect(canManageOrganizationMembers(OrganizationRole.Owner)).toBe(true);
      expect(canViewOrganizationMembers(OrganizationRole.Owner)).toBe(true);
    });

    it("grants Admin management permissions except deleting projects and organizations", () => {
      expect(canCreateProjects(OrganizationRole.Admin)).toBe(true);
      expect(canDeleteProjects(OrganizationRole.Admin)).toBe(false);
      expect(canManageOrganizationSettings(OrganizationRole.Admin)).toBe(true);
      expect(canViewRolledUpAudit(OrganizationRole.Admin)).toBe(true);
      expect(canManageOrganizationMembers(OrganizationRole.Admin)).toBe(true);
      expect(canViewOrganizationMembers(OrganizationRole.Admin)).toBe(true);
    });

    it("denies Member all organization-wide permissions", () => {
      expect(canCreateProjects(OrganizationRole.Member)).toBe(false);
      expect(canDeleteProjects(OrganizationRole.Member)).toBe(false);
      expect(canManageOrganizationSettings(OrganizationRole.Member)).toBe(false);
      expect(canViewRolledUpAudit(OrganizationRole.Member)).toBe(false);
      expect(canManageOrganizationMembers(OrganizationRole.Member)).toBe(false);
      expect(canViewOrganizationMembers(OrganizationRole.Member)).toBe(false);
    });

    it("denies null role all permissions", () => {
      expect(canCreateProjects(null)).toBe(false);
      expect(canDeleteProjects(null)).toBe(false);
      expect(canManageOrganizationSettings(null)).toBe(false);
      expect(canViewRolledUpAudit(null)).toBe(false);
      expect(canManageOrganizationMembers(null)).toBe(false);
      expect(canViewOrganizationMembers(null)).toBe(false);
    });
  });
});
