import { beforeEach, describe, expect, it, vi } from "vitest";

const mockRedirect = vi.fn();
vi.mock("next/navigation", () => ({
  redirect: (...args: unknown[]) => mockRedirect(...args),
}));
vi.mock("next/cache", () => ({
  revalidatePath: vi.fn(),
}));
vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
// The real `personHeaders`: which organization reaches auth — none — is the
// assertion under test.
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext: vi.fn(),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "invitation already used" })),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://auth.test",
  },
}));
const mockPublishEvent = vi.fn();
vi.mock("@/lib/events/publisher", () => ({
  publishEvent: (...args: unknown[]) => mockPublishEvent(...args),
  publishToAll: async (channels: (string | null | undefined)[], event: unknown) => {
    for (const channel of new Set(channels.filter(Boolean))) {
      await mockPublishEvent(channel, event);
    }
  },
  userChannel: (email: string) => `bfev:user:${email.toLowerCase()}`,
  organizationChannel: (organizationId: string) => `bfev:organization:${organizationId}`,
}));

import { revalidatePath } from "next/cache";
import { NextResponse } from "next/server";

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit } from "@/lib/api/rate-limit";
import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

import { acceptInviteAction } from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

describe("acceptInviteAction", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "usr_1",
      email: "user@example.test",
      accessToken: "at_1",
    } as never);
    vi.mocked(rateLimit).mockResolvedValue(null);
    vi.mocked(identityContext).mockResolvedValue({
      userId: "usr_1",
      organizationId: "org_1",
      role: "owner",
      accessToken: "at_1",
    });
    // Auth's answer to a PROJECT invitation's accept.
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({
          projectId: "project_1",
          inviteId: "inv_123",
          inviterEmail: "inviter@example.com",
          ownerOrganizationId: "org_owner",
        }),
        { status: 201 },
      ),
    );
  });

  it("redirects to login when session is absent", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null);
    await acceptInviteAction("token_abc");
    expect(mockRedirect).toHaveBeenCalledWith("/auth/login?returnTo=/invite/token_abc");
  });

  it("returns rate limit error when limited", async () => {
    vi.mocked(rateLimit).mockResolvedValue(new NextResponse(null, { status: 429 }));
    const res = await acceptInviteAction("token_abc");
    expect(res.error).toMatch(/too many requests/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("calls /internal/invites/accept and publishes invite:resolved", async () => {
    const res = await acceptInviteAction("token_abc");
    expect(res).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/invites/accept",
      expect.objectContaining({
        method: "POST",
        body: JSON.stringify({ token: "token_abc" }),
      }),
    );
    expect(mockPublishEvent).toHaveBeenCalledWith(
      "bfev:user:user@example.test",
      { type: "invite:resolved", data: { inviteId: "inv_123" } },
    );
    expect(mockPublishEvent).toHaveBeenCalledWith(
      "bfev:user:inviter@example.com",
      { type: "invite:resolved", data: { inviteId: "inv_123" } },
    );
    expect(mockPublishEvent).toHaveBeenCalledWith(
      "bfev:organization:org_owner",
      { type: "invite:resolved", data: { inviteId: "inv_123" } },
    );
    expect(vi.mocked(revalidatePath)).toHaveBeenCalledWith("/", "layout");
  });

  it("tells each stakeholder once when the inviter is the accepting person's own address", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({ inviteId: "inv_123", inviterEmail: "USER@example.test", ownerOrganizationId: "org_owner" }),
        { status: 201 },
      ),
    );
    await acceptInviteAction("token_abc");
    expect(mockPublishEvent).toHaveBeenCalledTimes(2);
  });

  it("returns problem message when upstream fails", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ error: "conflict" }), { status: 409 }),
    );
    const res = await acceptInviteAction("token_abc");
    expect(res.error).toBe("invitation already used");
  });

  // ⚠ **An invitation is how somebody in no organization gets into one.**
  // With sign-ups closed an invitee stands in none. It names no organization,
  // running under the person's bearer alone.
  it("accepts for somebody in no organization, naming none", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    expect(await acceptInviteAction("token_abc")).toEqual({ error: null });
    const [, init] = fetchMock.mock.calls[0]! as [string, { headers: Record<string, string> }];
    expect(init.headers.authorization).toBe("Bearer at_1");
    expect(init.headers).not.toHaveProperty("x-organization-id");
    expect(identityContext).not.toHaveBeenCalled();
  });
});
