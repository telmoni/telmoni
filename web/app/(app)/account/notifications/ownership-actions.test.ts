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
// The real `sessionHeaders`: what reaches auth is the assertion under test.
// `identityContext` answers with somewhere ELSE, so a lane that consulted it
// instead of the offer would send the wrong organization and fail below.
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext: vi.fn(async () => ({
    userId: "user_1",
    organizationId: "org_elsewhere",
    role: "owner",
    accessToken: "at_1",
  })),
}));
const mockRedirect = vi.fn((to: string) => {
  throw new Error(`REDIRECT:${to}`);
});
vi.mock("next/navigation", () => ({
  redirect: (to: string) => mockRedirect(to),
}));
const mockRevalidate = vi.fn();
vi.mock("next/cache", () => ({
  revalidatePath: (...a: unknown[]) => mockRevalidate(...a),
}));
const mockPublishEvent = vi.fn();
vi.mock("@/lib/events/publisher", () => ({
  publishEvent: (...args: unknown[]) => mockPublishEvent(...args),
  publishToAll: async (channels: (string | null | undefined)[], event: unknown) => {
    for (const channel of new Set(channels.filter(Boolean))) {
      await mockPublishEvent(channel, event);
    }
  },
  organizationChannel: (organizationId: string) => `bfev:organization:${organizationId}`,
}));

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { getServerSession } from "@/lib/server/session";

import {
  acceptOwnershipAction,
  acceptProjectOfferAction,
  declineOwnershipAction,
  declineProjectOfferAction,
} from "./ownership-actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "admin@example.test",
    accessToken: "at_1",
  } as never);
});

// ⚠ **An offer is answered from the notifications page, wherever the person
// happens to be standing.** The organization is the OFFER's, named by the
// caller — auth checks they hold a live offer there — and never the active
// one, which would accept or decline on behalf of the wrong organization.
describe.each([
  ["accept", acceptOwnershipAction],
  ["decline", declineOwnershipAction],
] as const)("answering an ownership offer: %s", (verb, answer) => {
  it("sends the organization the offer is for, not the one the console stands in", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await answer("org_offered")).toEqual({ error: null });

    expect(fetchMock).toHaveBeenCalledWith(
      `http://auth.test/internal/organization/owner-transfer/${verb}`,
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": "org_offered",
          "x-service-secret": "secret",
        }),
      }),
    );
    const init = fetchMock.mock.calls[0]![1] as { headers: Record<string, string> };
    expect(init.headers).not.toHaveProperty("x-user-id");
    expect(init).not.toHaveProperty("body");
  });

  // The previous owner and everybody else in the organization see the roles
  // move, and the offer leave the owner's roster.
  it("tells everyone in the organization once auth has answered", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    await answer("org_offered");

    expect(mockPublishEvent).toHaveBeenCalledTimes(1);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_offered", {
      type: "ownership:changed",
      data: { organizationId: "org_offered" },
    });
    expect(mockRevalidate).toHaveBeenCalledWith("/", "layout");
  });

  it("reports an unreachable service and a refusal distinctly, and tells nobody of either", async () => {
    fetchMock.mockResolvedValue(null);
    expect(await answer("org_offered")).toEqual({
      error: "Organizations are unavailable right now — try again shortly.",
    });

    fetchMock.mockResolvedValue(new Response(null, { status: 409 }));
    expect(await answer("org_offered")).toEqual({ error: "problem" });

    expect(mockPublishEvent).not.toHaveBeenCalled();
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  it("sends a signed-out caller to sign in, and asks auth nothing", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null);
    await expect(answer("org_offered")).rejects.toThrow("REDIRECT:/auth/login");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // The id becomes a header: anything but an organization id is refused
  // before auth is asked, rather than making the request itself fail.
  it("refuses anything but an organization id, and asks auth nothing", async () => {
    for (const bad of ["", "user_1", "org_offered\r\nx-service-secret: leaked"]) {
      expect(await answer(bad), JSON.stringify(bad)).toEqual({
        error: "That is not an organization.",
      });
    }
    expect(fetchMock).not.toHaveBeenCalled();
    expect(mockPublishEvent).not.toHaveBeenCalled();
  });
});

// ⚠ **The offer of a project is answered on the person's own lane.** The
// project is named in the path and where it lands in the body; no
// organization header travels, because the console may be standing anywhere
// — and the organization it stands in is not where the project is going.
describe("answering the offer of a project", () => {
  it("accepts into the organization named, follows the project there, and tells both organizations", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({
          projectId: "project_offered",
          slug: "payments-2",
          organizationId: "org_own",
          organizationSlug: "own",
          previousOrganizationId: "org_from",
          previousOwner: "user_2",
          name: "Payments 2",
        }),
        { status: 200 },
      ),
    );
    // The path names the organization the project landed in, and the slug it
    // landed under — which is auth's to say: a name taken there is numbered.
    expect(await acceptProjectOfferAction("project_offered", "org_own")).toEqual({
      error: null,
      href: "/own/payments-2",
    });

    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/projects/project_offered/transfer/accept",
      expect.objectContaining({
        method: "POST",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-service-secret": "secret",
        }),
        body: JSON.stringify({ organizationId: "org_own" }),
      }),
    );
    const init = fetchMock.mock.calls[0]![1] as { headers: Record<string, string> };
    expect(init.headers).not.toHaveProperty("x-organization-id");
    expect(init.headers).not.toHaveProperty("x-user-id");

    const changed = {
      type: "ownership:changed",
      data: { organizationId: "org_own", projectId: "project_offered" },
    };
    expect(mockPublishEvent).toHaveBeenCalledTimes(2);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_own", changed);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_from", changed);
    expect(mockRevalidate).toHaveBeenCalledWith("/", "layout");
  });

  it("answers nowhere to go when auth does not say where the project landed", async () => {
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({ projectId: "project_offered", organizationId: "org_own" }),
        { status: 200 },
      ),
    );
    expect(await acceptProjectOfferAction("project_offered", "org_own")).toEqual({
      error: null,
    });
  });

  it("declines, and tells the organization the project stays in", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ ownerOrganizationId: "org_from" }), { status: 200 }),
    );
    expect(await declineProjectOfferAction("project_offered")).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/projects/project_offered/transfer/decline",
      expect.objectContaining({ method: "POST", body: JSON.stringify({}) }),
    );
    expect(mockPublishEvent).toHaveBeenCalledTimes(1);
    expect(mockPublishEvent).toHaveBeenCalledWith("bfev:organization:org_from", {
      type: "ownership:changed",
      data: { organizationId: "org_from", projectId: "project_offered" },
    });
  });

  it("reports an unreachable service and a refusal distinctly, and moves nothing", async () => {
    fetchMock.mockResolvedValue(null);
    expect(await acceptProjectOfferAction("project_offered", "org_own")).toEqual({
      error: "Organizations are unavailable right now — try again shortly.",
    });

    fetchMock.mockResolvedValue(new Response(null, { status: 409 }));
    expect(await acceptProjectOfferAction("project_offered", "org_own")).toEqual({
      error: "problem",
    });

    expect(mockPublishEvent).not.toHaveBeenCalled();
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  // Both ids reach auth in the URL or the body: anything but the shape each
  // has is refused before auth is asked.
  it("refuses anything but a project and an organization id, and asks auth nothing", async () => {
    for (const bad of ["", "org_1", "project_offered\r\nx: leaked"]) {
      expect(await acceptProjectOfferAction(bad, "org_own"), JSON.stringify(bad)).toEqual({
        error: "That is not a project.",
      });
    }
    expect(await acceptProjectOfferAction("project_offered", "user_1")).toEqual({
      error: "That is not an organization.",
    });
    expect(await declineProjectOfferAction("org_1")).toEqual({
      error: "That is not a project.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
    expect(mockPublishEvent).not.toHaveBeenCalled();
  });
});
