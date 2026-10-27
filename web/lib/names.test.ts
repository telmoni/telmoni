import { describe, expect, it } from "vitest";

import {
  MAX_ORGANIZATION_NAME,
  MAX_PROJECT_NAME,
  checkOrganizationName,
  checkOrganizationUrl,
  checkProjectName,
  typedBound,
} from "./names";

describe("a typed name", () => {
  // Auth counts characters, so 40 emoji and 30 letters are 70 of them,
  // though 110 UTF-16 units.
  it("is counted in characters, as auth counts it", () => {
    const name = "🦀".repeat(40) + "a".repeat(30);
    expect(name.length).toBe(110);
    expect(checkProjectName(name)).toEqual({ ok: true, value: name });
    expect(checkProjectName("a".repeat(MAX_PROJECT_NAME + 1))).toEqual({
      ok: false,
      error: "Project name must be 100 characters or fewer.",
    });
    expect(checkOrganizationName("🦀".repeat(MAX_ORGANIZATION_NAME)).ok).toBe(true);
    expect(checkOrganizationName("🦀".repeat(MAX_ORGANIZATION_NAME + 1)).ok).toBe(false);
  });

  it("is trimmed, and refused when nothing is left", () => {
    expect(checkProjectName("  Payments  ")).toEqual({ ok: true, value: "Payments" });
    expect(checkProjectName("   ")).toEqual({ ok: false, error: "Project name cannot be empty." });
    expect(checkOrganizationName("")).toEqual({
      ok: false,
      error: "Give your organization a name.",
    });
  });

  // An input's own bound counts code units, so it must admit every name the
  // rule does.
  it("is bounded at the input loosely enough for any name the rule takes", () => {
    expect(typedBound(MAX_PROJECT_NAME)).toBeGreaterThanOrEqual("🦀".repeat(MAX_PROJECT_NAME).length);
  });
});

describe("an organization's URL", () => {
  it("is held to its shape, and to the console's own words", () => {
    expect(checkOrganizationUrl(" acme-labs ")).toEqual({ ok: true, value: "acme-labs" });
    expect(checkOrganizationUrl("Acme Labs").ok).toBe(false);
    expect(checkOrganizationUrl("settings")).toEqual({
      ok: false,
      error: "settings is a word the console's own pages use — choose another.",
    });
  });
});
