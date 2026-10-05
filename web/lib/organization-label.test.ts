import { describe, expect, it } from "vitest";

import { resolveActiveOrganization } from "./organization-label";

const MINE = "org_mine";
const THEIRS = "org_theirs";

const mine = {
  organizationId: MINE,
  name: "Acme",
  ownerEmail: "ada@example.com" as string | null,
  ownerDisplayName: "Ada",
  role: "owner" as const,
};

const theirs = {
  organizationId: THEIRS,
  name: "Analytical Engines",
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
  // ⚠ The name the organization goes by, and nothing the console makes up from
  // the person who holds it: their address is theirs, and a path and a mail
  // would carry it.
  it("is the organization's name, never its owner's name or address", () => {
    expect(resolve().label).toBe("Acme");
    expect(resolve().label).not.toContain("Ada");
    expect(resolve().label).not.toContain("ada@");
  });

  it("names somebody else's organization by its own name", () => {
    expect(resolve({ activeOrganizationId: THEIRS }).label).toBe("Analytical Engines");
    expect(resolve({ activeOrganizationId: THEIRS }).label).not.toContain("Grace");
  });

  it("says Organization when there is no organization to name", () => {
    expect(resolve({ activeOrganizationId: "org_stranger" }).label).toBe("Organization");
    expect(resolve({ organizations: [] }).label).toBe("Organization");
  });
});
