import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "problem" })),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://auth.test",
    SERVICE_SECRET: "secret",
  },
}));
// The real `organizationHeaders`: what reaches the server is the assertion.
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext: vi.fn(),
}));
vi.mock("@/lib/server/entities/organization", () => ({
  getServerContext: vi.fn(async () => null),
}));
vi.mock("next/navigation", () => ({
  redirect: (to: string) => {
    throw new Error(`REDIRECT:${to}`);
  },
}));

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerContext } from "@/lib/server/entities/organization";
import { getServerSession } from "@/lib/server/session";

import { markOrganizationReadAction } from "./notice-actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

// The organization the overview rendered, and handed to the button.
const ORGANIZATION = "org_acme";

function standingIn(organizationId: string) {
  vi.mocked(identityContext).mockResolvedValue({
    userId: "user_1",
    organizationId,
    role: "admin",
    accessToken: "at_1",
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "user@example.test",
  } as never);
});

describe("markOrganizationReadAction", () => {
  it("marks the feed of the organization the page rendered read", async () => {
    standingIn(ORGANIZATION);
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await markOrganizationReadAction(ORGANIZATION)).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/notifications/read",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({ "x-organization-id": ORGANIZATION }),
      }),
    );
  });

  // The page said A; the request resolves B. Marking B's feed read from A's
  // overview would be wrong even for something this small.
  it("refuses, asking the server nothing, when the request resolves another organization", async () => {
    standingIn("org_elsewhere");
    const res = await markOrganizationReadAction(ORGANIZATION);
    expect(res.error).toMatch(/out of date/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // Posted from a tab whose organization was renamed, or let the person go,
  // after the page rendered: a reload would answer "not found".
  it("tells a tab that missed a move that its address is gone", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    vi.mocked(getServerContext).mockResolvedValue({
      organizationNotFound: true,
    } as Awaited<ReturnType<typeof getServerContext>>);
    const res = await markOrganizationReadAction(ORGANIZATION);
    expect(res.error).toMatch(/renamed, or you're no longer in it/);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
