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


describe("ownedOrganizationLabels", () => {
  it("names the organizations the person owns, and only those", () => {
    expect(
      ownedOrganizationLabels([
        { name: "Acme", role: "owner" },
        { name: "Theirs", role: "admin" },
        { name: null, role: "member" },
      ]),
    ).toEqual(["Acme"]);
  });

  // Two organizations may carry one name, and must not read as one.
  it("counts the ones that share a label instead of repeating it", () => {
    expect(
      ownedOrganizationLabels([
        { name: "Acme", role: "owner" },
        { name: "Globex", role: "owner" },
        { name: "Acme", role: "owner" },
      ]),
    ).toEqual(["Acme (2 organizations)", "Globex"]);
  });

  it("is empty for somebody who owns nothing", () => {
    expect(ownedOrganizationLabels([])).toEqual([]);
  });
});
