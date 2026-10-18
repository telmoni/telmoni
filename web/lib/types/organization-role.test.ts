import { describe, expect, it } from "vitest";

import { OrganizationRole, asOrganizationRole } from "./organization-role";

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
});
