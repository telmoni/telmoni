import { describe, expect, it } from "vitest";

import { accountOnly } from "./account-only";

describe("accountOnly", () => {
  it("is the console for somebody standing in an organization", () => {
    expect(accountOnly({ activeOrganizationId: "org_1" })).toBe(false);
  });

  it("is the account alone for somebody in no organization", () => {
    expect(accountOnly({ activeOrganizationId: null })).toBe(true);
  });
});
