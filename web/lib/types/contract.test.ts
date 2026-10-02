import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  asRole,
  Flag,
  FlagOffDetail,
  NotificationKind,
  OrganizationStatus,
  Role,
} from "./enums";
import { RESERVED_ORGANIZATION_SLUGS, SLUG_MAX_LENGTH } from "../slug";

const contract = JSON.parse(
  readFileSync(
    fileURLToPath(new URL("../../../contract/wire-contract.json", import.meta.url)),
    "utf8",
  ),
) as {
  enums: Record<string, string[]>;
  flags: { off_detail: Record<string, string> };
  slugs: { max_length: number; reserved: string[] };
};

// Every enum the server publishes, and the console's copy of it. A new one
// in the contract fails the first test until it is added here.
const CONSOLE: Record<string, Record<string, string>> = {
  Flag,
  NotificationKind,
  OrganizationStatus,
  Role,
};

describe("wire contract", () => {
  it("the console knows every enum the server publishes", () => {
    expect(Object.keys(CONSOLE).sort()).toEqual(Object.keys(contract.enums).sort());
  });

  it.each(Object.keys(CONSOLE))("%s has exactly the server's values", (name) => {
    expect(Object.values(CONSOLE[name]!).sort()).toEqual([...contract.enums[name]!].sort());
  });

  it("each switched-off feature says what the server says", () => {
    expect(FlagOffDetail).toEqual(contract.flags.off_detail);
  });

  // The proxy reads a path's first segment as an organization unless it is
  // one of these; a word auth reserves and the console does not would be a
  // page of its own that the proxy took for an organization.
  it("the console reserves exactly the words the server refuses an organization", () => {
    expect([...RESERVED_ORGANIZATION_SLUGS].sort()).toEqual([...contract.slugs.reserved].sort());
    expect(SLUG_MAX_LENGTH).toBe(contract.slugs.max_length);
  });

  it("the role coercion takes the three roles and nothing else", () => {
    expect(asRole("editor")).toBeNull();
    expect(asRole("viewer")).toBeNull();
    expect(asRole("guest")).toBeNull();

    expect(asRole("owner")).toBe(Role.Owner);
    expect(asRole("admin")).toBe(Role.Admin);
    expect(asRole("member")).toBe(Role.Member);
  });
});
