import { beforeEach, describe, expect, it, vi } from "vitest";

import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerContext } from "@/lib/server/entities/organization";

import { activeProjectForMutation } from "./identity";

vi.mock("@/lib/server/entities/identity-context", () => ({
  identityContext: vi.fn(),
}));
vi.mock("@/lib/server/entities/organization", () => ({
  getServerContext: vi.fn(async () => null),
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

  // The path this was posted from names an organization auth did not answer
  // with: a URL change this tab missed, or a membership that ended. A reload would
  // find the same address gone, so "try again" would be the wrong advice.
  it("tells a tab that missed a move that its address is gone", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    vi.mocked(getServerContext).mockResolvedValue({
      organizationNotFound: true,
    } as Awaited<ReturnType<typeof getServerContext>>);
    const active = await activeProjectForMutation("project_1");
    expect(active.organizationId).toBeUndefined();
    expect(active.error).toMatch(/URL was changed, or you're no longer in it/);
    expect(active.error).not.toMatch(/try again/i);
  });
});
