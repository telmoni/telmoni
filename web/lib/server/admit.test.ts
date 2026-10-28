import { beforeEach, describe, expect, it, vi } from "vitest";

import { SESSION_EXPIRED, TOO_MANY_REQUESTS } from "@/lib/action-result";
import { rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerContext } from "@/lib/server/entities/organization";
import { getServerSession } from "@/lib/server/session";

import { admit, admitOrganization, admitProject } from "./admit";
import { SWITCHED_ORGANIZATION } from "./identity";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: (_: unknown, call: string) => `user_self:${call}`,
}));
vi.mock("@/lib/server/entities/identity-context", () => ({
  identityContext: vi.fn(),
  sessionHeaders: (_: unknown, organizationId: string, projectId?: string) => ({
    "x-organization-id": organizationId,
    ...(projectId ? { "x-project-id": projectId } : {}),
  }),
}));
vi.mock("@/lib/server/entities/organization", () => ({
  getServerContext: vi.fn(async () => null),
}));

const SESSION = { userId: "user_self", email: "self@example.test", accessToken: "at_self" };

// Standing in an organization the caller administers and does not own.
const STANDING = {
  userId: "user_self",
  organizationId: "org_theirs",
  role: "admin" as const,
  accessToken: "at_self",
};

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue(
    SESSION as Awaited<ReturnType<typeof getServerSession>>,
  );
  vi.mocked(identityContext).mockResolvedValue(STANDING);
  vi.mocked(rateLimitRetryAfter).mockResolvedValue(null);
});

describe("admit", () => {
  it("refuses a lapsed session before it counts anything", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null);
    expect(await admit({ call: "x", limit: 1 })).toEqual({
      ok: false,
      error: SESSION_EXPIRED,
      status: 401,
    });
    expect(rateLimitRetryAfter).not.toHaveBeenCalled();
  });

  it("counts the call against its own ceiling, a minute by default", async () => {
    expect(await admit({ call: "tokens:mint", limit: 5 })).toMatchObject({ ok: true });
    expect(rateLimitRetryAfter).toHaveBeenCalledWith("user_self:tokens:mint", {
      limit: 5,
      windowMs: 60_000,
    });
  });

  it("says when to try again, in the window's own words", async () => {
    vi.mocked(rateLimitRetryAfter).mockResolvedValue(12);
    expect(await admit({ call: "x", limit: 1 })).toEqual({
      ok: false,
      error: TOO_MANY_REQUESTS,
      status: 429,
      retryAfter: 12,
    });
    const hourly = await admit({ call: "x", limit: 1, per: "hour" });
    expect(hourly).toMatchObject({ status: 429, error: expect.stringMatching(/in an hour/) });
    expect(rateLimitRetryAfter).toHaveBeenLastCalledWith("user_self:x", {
      limit: 1,
      windowMs: 3_600_000,
    });
  });
});

describe("admitProject", () => {
  it("pairs the project with the organization the caller is standing in", async () => {
    expect(await admitProject({ call: "x", limit: 1 }, "project_1")).toMatchObject({
      ok: true,
      organizationId: "org_theirs",
      projectId: "project_1",
      headers: { "x-organization-id": "org_theirs", "x-project-id": "project_1" },
    });
  });

  // The id becomes a header: a Server Action is handed whatever was posted.
  it("refuses anything but a project id before it asks auth anything", async () => {
    for (const id of ["", "project_1\r\nx-service-secret: leaked", "../organization", "a/b"]) {
      const gate = await admitProject({ call: "x", limit: 1 }, id);
      expect(gate, JSON.stringify(id)).toMatchObject({ ok: false, status: 404 });
      expect(gate.ok ? null : gate.error).toMatch(/resolve that project/i);
    }
    expect(identityContext).not.toHaveBeenCalled();
  });

  it("refuses rather than guessing when auth is unreachable", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    const gate = await admitProject({ call: "x", limit: 1 }, "project_1");
    expect(gate.ok ? null : gate.error).toMatch(/try again/i);
  });

  // The path this was posted from names an organization auth did not answer
  // with: a reload would find the same address gone, so "try again" is wrong.
  it("tells a tab that missed a move that its address is gone", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    vi.mocked(getServerContext).mockResolvedValue({
      organizationNotFound: true,
    } as Awaited<ReturnType<typeof getServerContext>>);
    const gate = await admitProject({ call: "x", limit: 1 }, "project_1");
    const error = gate.ok ? null : gate.error;
    expect(error).toMatch(/URL was changed, or you're no longer in it/);
    expect(error).not.toMatch(/try again/i);
  });
});

describe("admitOrganization", () => {
  const OWNER_ONLY = { role: "owner" as const, refused: "Only the owner." };

  it("lets in the role the call needs, in the organization the page rendered", async () => {
    const gate = await admitOrganization({ call: "x", limit: 1 }, "org_theirs", {
      role: "admin",
      refused: "Only an admin.",
    });
    expect(gate).toMatchObject({ ok: true, ctx: STANDING });
  });

  it("refuses a role short of the call's, in the call's words", async () => {
    expect(await admitOrganization({ call: "x", limit: 1 }, "org_theirs", OWNER_ONLY)).toEqual({
      ok: false,
      error: "Only the owner.",
      status: 403,
    });
  });

  // A page out of date is told so, never that the person lacks a role they
  // may well hold in the organization the page shows.
  it("checks the organization before the role", async () => {
    expect(await admitOrganization({ call: "x", limit: 1 }, "org_shown", OWNER_ONLY)).toEqual({
      ok: false,
      error: SWITCHED_ORGANIZATION,
      status: 409,
    });
  });
});
