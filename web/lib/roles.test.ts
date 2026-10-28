import { describe, expect, it } from "vitest";

import { administers, isGivenRole, organizationRoleOf, owns } from "./roles";

const ctx = {
  organizations: [
    { organizationId: "org_mine", role: "owner" as const },
    { organizationId: "org_theirs", role: "admin" as const },
    { organizationId: "org_seat", role: "member" as const },
  ],
};

describe("organizationRoleOf", () => {
  it("reads the role off the entry for the organization named", () => {
    expect(organizationRoleOf(ctx, { organizationId: "org_mine" })).toBe("owner");
    expect(organizationRoleOf(ctx, { organizationId: "org_theirs" })).toBe("admin");
    expect(organizationRoleOf(ctx, { organizationId: "org_seat" })).toBe("member");
  });

  // ⚠ An id that happens to equal the caller's user id is no organization
  // of theirs: with no entry for it, it carries no role at all.
  it("grants nothing for an organization the list does not hold, whatever its id", () => {
    const ident = { userId: "user_1", organizationId: "user_1" };
    expect(organizationRoleOf(ctx, ident)).toBeNull();
    expect(organizationRoleOf(ctx, { organizationId: "org_stranger" })).toBeNull();
  });

  // A degraded `/me` answers "no role" rather than a 500 on a page whose whole
  // job is to decide what somebody may see.
  it("answers no role when there is nobody asking or no list to read", () => {
    expect(organizationRoleOf(ctx, null)).toBeNull();
    expect(organizationRoleOf(null, { organizationId: "org_mine" })).toBeNull();
    expect(organizationRoleOf({}, { organizationId: "org_mine" })).toBeNull();
  });
});

describe("administers", () => {
  // One rule for both ladders: a project's and an organization's.
  it("is the owner and the admins, and nobody else", () => {
    expect(administers("owner")).toBe(true);
    expect(administers("admin")).toBe(true);
    expect(administers("member")).toBe(false);
    expect(administers(null)).toBe(false);
    expect(administers(undefined)).toBe(false);
  });
});

describe("owns", () => {
  it("is the owner alone", () => {
    expect(owns("owner")).toBe(true);
    expect(owns("admin")).toBe(false);
    expect(owns(null)).toBe(false);
  });
});

describe("isGivenRole", () => {
  // Ownership is handed over, never given.
  it("is admin and member, never owner nor anything posted", () => {
    expect(isGivenRole("admin")).toBe(true);
    expect(isGivenRole("member")).toBe(true);
    expect(isGivenRole("owner")).toBe(false);
    expect(isGivenRole("editor")).toBe(false);
    expect(isGivenRole(["admin"])).toBe(false);
  });
});
