import { beforeEach, describe, expect, it, vi } from "vitest";

import { identityContext } from "@/lib/server/entities/identity-context";

import { activeProjectForMutation } from "./identity";

vi.mock("@/lib/server/entities/identity-context", () => ({
  identityContext: vi.fn(),
}));

// Standing in an organization the caller administers and does not own — the
// case the old answer got wrong.
const STANDING = {
  userId: "user_self",
  organizationId: "org_theirs",
  role: "admin" as const,
  accessToken: "at_self",
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(identityContext).mockResolvedValue(STANDING);
});

describe("activeProjectForMutation", () => {
  it("pairs the project with the organization the caller is standing in", async () => {
    expect(await activeProjectForMutation("project_1")).toEqual({
      organizationId: "org_theirs",
      projectId: "project_1",
      error: null,
    });
  });

  it("refuses a missing project before it asks auth anything", async () => {
    const active = await activeProjectForMutation("");
    expect(active.projectId).toBeUndefined();
    expect(active.error).toMatch(/which project/i);
    expect(identityContext).not.toHaveBeenCalled();
  });

  it("refuses rather than guessing when auth is unreachable", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    const active = await activeProjectForMutation("project_1");
    expect(active.organizationId).toBeUndefined();
    expect(active.error).toMatch(/try again/i);
  });
});
