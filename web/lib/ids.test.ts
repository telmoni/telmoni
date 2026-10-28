import { describe, expect, it } from "vitest";

import { isOrganizationId, isProjectId } from "./ids";

describe("ids", () => {
  it("takes each kind of id by its own prefix, and no other kind", () => {
    expect(isProjectId("project_1")).toBe(true);
    expect(isOrganizationId("org_1")).toBe(true);
    expect(isProjectId("org_1")).toBe(false);
    expect(isOrganizationId("project_1")).toBe(false);
  });

  // An id becomes a header or a path segment: nothing that could aim either.
  it("refuses what could reach past its segment, and anything but a string", () => {
    for (const bad of ["project_../x", "project_a/b", "project_a\r\nx: y", "", "project_" + "x".repeat(64)]) {
      expect(isProjectId(bad), bad).toBe(false);
    }
    expect(isProjectId(["project_1"])).toBe(false);
    expect(isOrganizationId(42)).toBe(false);
  });
});
