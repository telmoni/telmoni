import { describe, expect, it } from "vitest";

import { resolveActiveOrganization } from "./organization-label";

const MINE = "org_mine";
const THEIRS = "org_theirs";

const mine = {
  organizationId: MINE,
  name: null as string | null,
  ownerEmail: "ada@example.com" as string | null,
  ownerDisplayName: "Ada",
  role: "owner" as const,
};

const theirs = {
  organizationId: THEIRS,
  name: null as string | null,
  ownerEmail: "grace@other.example" as string | null,
  ownerDisplayName: "Grace H",
  role: "admin" as const,
};

function resolve(
  over: Partial<Parameters<typeof resolveActiveOrganization>[0]> = {},
) {
  return resolveActiveOrganization({
    activeOrganizationId: MINE,
    organizations: [mine, theirs],
    ...over,
  });
}

describe("which organization you are in", () => {
  it("is the entry the active id names, one you own", () => {
    expect(resolve().active).toBe(mine);
  });

  it("is the entry the active id names, one you only belong to", () => {
    expect(resolve({ activeOrganizationId: THEIRS }).active).toBe(theirs);
  });

  // ⚠ Nothing active used to mean your OWN organization — the one whose id was
  // your user id. There is no such organization any more, so there is nothing
  // to fall back to, and a guess would name a tenant auth did not resolve.
  it("is nowhere when nothing is active, rather than one you own", () => {
    expect(resolve({ activeOrganizationId: null }).active).toBeNull();
  });

  it("is nowhere when the active id names an organization you are not in", () => {
    expect(resolve({ activeOrganizationId: "org_stranger" }).active).toBeNull();
  });
});

describe("what to call it", () => {
  it("prefers the name the owner chose", () => {
    expect(
      resolve({ organizations: [{ ...mine, name: "Acme Robotics" }, theirs] }).label,
    ).toBe("Acme Robotics");
  });

  // ⚠ The regression this file exists for. `name` is NULL until the owner
  // names it, so the label comes off the owner's LIVE address — change it, or
  // hand the organization to somebody else, and every surface follows, where a
  // seeded copy of the old address would not.
  it("falls back to the owner's live address when nobody has named it", () => {
    expect(resolve().label).toBe("ada@example.com");
    expect(
      resolve({ organizations: [{ ...mine, ownerEmail: "ada@new.example" }, theirs] }).label,
    ).toBe("ada@new.example");
  });

  it("treats a blank name as no name", () => {
    expect(resolve({ organizations: [{ ...mine, name: "   " }, theirs] }).label).toBe(
      "ada@example.com",
    );
  });

  // An organization is named by its name or its owner's address, never by the
  // person who holds it — and "Personal" is gone, because it was one word for
  // every customer.
  it("never labels an organization with a person's name", () => {
    expect(resolve().label).not.toBe("Personal");
    expect(resolve().label).not.toContain("Ada");
    expect(resolve({ activeOrganizationId: THEIRS }).label).toBe("grace@other.example");
    expect(resolve({ activeOrganizationId: THEIRS }).label).not.toContain("Grace");
  });

  // Every entry carries its organization's own name now. Only yours did, so
  // somebody else's organization could only ever be shown by its address.
  it("names somebody else's organization by the name its owner chose", () => {
    expect(
      resolve({
        activeOrganizationId: THEIRS,
        organizations: [mine, { ...theirs, name: "Analytical Engines" }],
      }).label,
    ).toBe("Analytical Engines");
  });

  it("says Organization when there is nothing to name it with", () => {
    expect(resolve({ activeOrganizationId: "org_stranger" }).label).toBe("Organization");
    expect(resolve({ organizations: [] }).label).toBe("Organization");
    // An entry read mid-transfer has no owner row to take an address from.
    expect(resolve({ organizations: [{ ...mine, ownerEmail: null }] }).label).toBe(
      "Organization",
    );
  });
});
