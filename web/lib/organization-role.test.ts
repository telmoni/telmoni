import { describe, expect, it } from "vitest";

import { administersOrganization, organizationRoleOf } from "./organization-role";

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

describe("administersOrganization", () => {
  it("is the owner and the admins, and nobody else", () => {
    expect(administersOrganization("owner")).toBe(true);
    expect(administersOrganization("admin")).toBe(true);
    expect(administersOrganization("member")).toBe(false);
    expect(administersOrganization(null)).toBe(false);
  });
});
