import { describe, expect, it } from "vitest";

import { accountOnlyReason } from "./account-only";

describe("accountOnlyReason", () => {
  it("is the console for somebody standing in an organization in the beta", () => {
    expect(accountOnlyReason({ activeOrganizationId: "org_1", flags: {} })).toBeNull();
    expect(
      accountOnlyReason({ activeOrganizationId: "org_1", flags: { beta_access: true } }),
    ).toBeNull();
  });

  it("is the account alone for somebody in no organization", () => {
    expect(accountOnlyReason({ activeOrganizationId: null, flags: {} })).toBe(
      "no-organization",
    );
  });

  // Checked first: somebody with no organization has nothing the beta could
  // let them into, and the page should say what they can do about it.
  it("names the missing organization before the beta, when both are true", () => {
    expect(
      accountOnlyReason({ activeOrganizationId: null, flags: { beta_access: false } }),
    ).toBe("no-organization");
  });

  it("is the account alone for somebody whose organizations are not in the beta", () => {
    expect(
      accountOnlyReason({ activeOrganizationId: "org_1", flags: { beta_access: false } }),
    ).toBe("not-in-beta");
  });
});
