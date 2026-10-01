import { describe, expect, it } from "vitest";

import { roleLabel } from "./role-label";
import { OrganizationRole } from "./types/organization-role";
import { Role } from "./types/enums";

describe("roleLabel", () => {
  // ⚠ **Both ladders, on purpose.** This was six private copies, one of them
  // named `organizationRoleLabel` and living beside the organization enum —
  // which is what made the next person write a seventh for project roles rather
  // than reach for a function whose name said it was not theirs.
  it("titles an organization role", () => {
    expect(roleLabel(OrganizationRole.Owner)).toBe("Owner");
    expect(roleLabel(OrganizationRole.Admin)).toBe("Admin");
    expect(roleLabel(OrganizationRole.Member)).toBe("Member");
  });

  it("titles a project role", () => {
    expect(roleLabel(Role.Owner)).toBe("Owner");
    expect(roleLabel(Role.Admin)).toBe("Admin");
    expect(roleLabel(Role.Member)).toBe("Member");
  });

  // Every copy answered an em dash for an absent role, and every call site
  // relied on it — one of them wrapped the call in its own `? :` to say the
  // same thing twice.
  it("answers an em dash rather than an empty cell", () => {
    expect(roleLabel(null)).toBe("—");
    expect(roleLabel(undefined)).toBe("—");
    expect(roleLabel("")).toBe("—");
  });
});
