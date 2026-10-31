import { describe, expect, it } from "vitest";

import { memberNames, ownedOrganizationCount, ownerContact } from "./identity";

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

describe("memberNames", () => {
  // ⚠ An organization's admin acts on every project with no seat on it.
  it("names a member from the rosters read, and says what the rest may be", () => {
    const ada = { memberId: "user_1", displayName: "Ada", email: "ada@example.test" };
    const sam = { memberId: "user_2", displayName: null, email: "sam@example.test" };
    const grace = { memberId: "user_3", displayName: "Grace", email: "grace@example.test" };
    const both = memberNames([grace], [ada, sam]);
    expect([both("user_1"), both("user_2"), both("user_3"), both("user_9")]).toEqual([
      "Ada",
      "sam@example.test",
      "Grace",
      "a former member",
    ]);
    expect(memberNames(null, [ada])("user_9")).toBe("an organization admin or a former member");
    expect(memberNames(null, null)("user_1")).toBe("someone");
  });
});
