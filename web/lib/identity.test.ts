import { describe, expect, it } from "vitest";

import { ownedOrganizationLabels, ownerContact } from "./identity";

describe("ownerContact", () => {
  it("names the owner, as a member row would", () => {
    expect(
      ownerContact({ ownerEmail: "ada@example.com", ownerDisplayName: "Ada Lovelace" }),
    ).toBe("Ada Lovelace");
  });

  it("falls back to the owner's address, never to nothing", () => {
    expect(ownerContact({ ownerEmail: "ada@example.com", ownerDisplayName: null })).toBe(
      "ada@example.com",
    );
    expect(ownerContact({ ownerEmail: "ada@example.com", ownerDisplayName: "  " })).toBe(
      "ada@example.com",
    );
  });

  it("is null when /me carried no owner", () => {
    expect(ownerContact({ ownerEmail: null })).toBeNull();
    expect(ownerContact({})).toBeNull();
    expect(ownerContact(null)).toBeNull();
    expect(ownerContact(undefined)).toBeNull();
  });
});

const MINE = "ada@example.test";

describe("ownedOrganizationLabels", () => {
  it("names the organizations the person owns, and only those", () => {
    expect(
      ownedOrganizationLabels([
        { name: "Acme", ownerEmail: MINE, role: "owner" },
        { name: "Theirs", ownerEmail: "grace@example.test", role: "admin" },
        { name: null, ownerEmail: "linus@example.test", role: "member" },
      ]),
    ).toEqual(["Acme"]);
  });

  // Every unnamed organization wears its owner's address, so two of them
  // would otherwise read as the same organization named twice.
  it("counts the ones that share a label instead of repeating it", () => {
    expect(
      ownedOrganizationLabels([
        { name: null, ownerEmail: MINE, role: "owner" },
        { name: "Acme", ownerEmail: MINE, role: "owner" },
        { name: "  ", ownerEmail: MINE, role: "owner" },
      ]),
    ).toEqual([`${MINE} (2 organizations)`, "Acme"]);
  });

  it("is empty for somebody who owns nothing", () => {
    expect(ownedOrganizationLabels([])).toEqual([]);
  });
});
