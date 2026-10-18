import { describe, expect, it } from "vitest";

import { administersProject } from "./project-role";

describe("administersProject", () => {
  it("is the owner and the admins, and nobody else", () => {
    expect(administersProject("owner")).toBe(true);
    expect(administersProject("admin")).toBe(true);
    expect(administersProject("member")).toBe(false);
    expect(administersProject(null)).toBe(false);
  });
});
