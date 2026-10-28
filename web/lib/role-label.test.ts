import { describe, expect, it } from "vitest";

import { roleLabel } from "./role-label";
import { OrganizationRole, Role } from "./types/enums";

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
});
