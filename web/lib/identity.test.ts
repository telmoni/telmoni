import { describe, expect, it } from "vitest";

import { ownedOrganizationCount, ownerContact } from "./identity";

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


describe("ownedOrganizationCount", () => {
  it("counts the organizations the person owns, and only those", () => {
    expect(
      ownedOrganizationCount([
        { role: "owner" },
        { role: "admin" },
        { role: "member" },
        { role: "owner" },
      ]),
    ).toBe(2);
  });

  it("is nothing for somebody who owns nothing", () => {
    expect(ownedOrganizationCount([])).toBe(0);
    expect(ownedOrganizationCount([{ role: "admin" }])).toBe(0);
  });
});
