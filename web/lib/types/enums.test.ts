import { describe, expect, it } from "vitest";

import { asRole, Role } from "./enums";

describe("asRole", () => {
  it("passes through known wire roles", () => {
    expect(asRole("owner")).toBe(Role.Owner);
    expect(asRole("admin")).toBe(Role.Admin);
    expect(asRole("member")).toBe(Role.Member);
  });

  it("fails closed (null) for unknown / missing roles", () => {
    expect(asRole("superuser")).toBeNull();
    expect(asRole("readonly")).toBeNull();
    expect(asRole("editor")).toBeNull();
    expect(asRole("read_only")).toBeNull();
    expect(asRole("viewer")).toBeNull();
    expect(asRole("developer")).toBeNull();
    expect(asRole("security")).toBeNull();
    expect(asRole("contributor")).toBeNull();
    expect(asRole(undefined)).toBeNull();
    expect(asRole(null)).toBeNull();
    expect(asRole("")).toBeNull();
  });
});
