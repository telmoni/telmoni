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
// The real `organizationHeaders`: what reaches auth is the assertion under test.
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext: vi.fn(),
}));
// `/me`'s answer for the request, which is where the action reads the slug the
// organization went by before the rename.
vi.mock("@/lib/server/entities/organization", () => ({
  getServerContext: vi.fn(),
  activeOrganization: (ctx: {
    activeOrganizationId: string;
    organizations: { organizationId: string }[];
  }) => ctx.organizations.find((o) => o.organizationId === ctx.activeOrganizationId) ?? null,
}));
vi.mock("next/navigation", () => ({
  redirect: (to: string) => {
    throw new Error(`REDIRECT:${to}`);
  },
}));
const mockRevalidate = vi.fn();
vi.mock("next/cache", () => ({
  revalidatePath: (...a: unknown[]) => mockRevalidate(...a),
}));
const mockPublishEvent = vi.fn<(...args: unknown[]) => Promise<boolean>>(async () => true);
vi.mock("@/lib/events/publisher", () => ({
  publishEvent: (...args: unknown[]) => mockPublishEvent(...args),
  organizationChannel: (organizationId: string) => `bfev:organization:${organizationId}`,
}));

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerContext } from "@/lib/server/entities/organization";
import { getServerSession } from "@/lib/server/session";

import { renameOrganizationAction } from "./rename-actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

// The organization the settings page rendered, and handed to the form.
const ORGANIZATION = "org_acme";
const SWITCHED =
  "This page is out of date. Reload it to act on the organization you're viewing.";

// Auth's answer to a rename: the name as stored, and the slug it goes by now.
const renamed = (slug: string) =>
  new Response(JSON.stringify({ name: "Acme Robotics", slug }), { status: 200 });

function wentBy(slug: string) {
  vi.mocked(getServerContext).mockResolvedValue({
    activeOrganizationId: ORGANIZATION,
    organizations: [{ organizationId: ORGANIZATION, slug }],
  } as never);
}

function standingIn(organizationId: string) {
  vi.mocked(identityContext).mockResolvedValue({
    userId: "user_1",
    organizationId,
    role: "owner",
    accessToken: "at_1",
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "owner@example.test",
    accessToken: "at_1",
  } as never);
  standingIn(ORGANIZATION);
  wentBy("acme-robotics");
});

describe("renameOrganizationAction", () => {
  it("renames the organization the page rendered, under the session's bearer", async () => {
    fetchMock.mockResolvedValue(renamed("acme-robotics"));
    expect(await renameOrganizationAction(ORGANIZATION, "  Acme Robotics  ")).toEqual({
      error: null,
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/organization/name",
      expect.objectContaining({
        method: "PUT",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": ORGANIZATION,
          "content-type": "application/json",
        }),
        body: JSON.stringify({ name: "Acme Robotics" }),
      }),
    );
    expect(mockRevalidate).toHaveBeenCalledWith("/", "layout");
    // Its paths are where they were: nobody has anywhere to follow it to.
    expect(mockPublishEvent).not.toHaveBeenCalled();
  });

  // ⚠ A rename moves the organization's slug with its name, out from under
  // the path this was posted from. The page follows it; rendering the old path
  // again first, as a revalidation would, answers "not found".
  it("says where a rename moved the organization, and revalidates nothing", async () => {
    wentBy("org-4k2j9x0q1z");
    fetchMock.mockResolvedValue(renamed("acme-robotics"));
    expect(await renameOrganizationAction(ORGANIZATION, "Acme Robotics")).toEqual({
      error: null,
      movedTo: "acme-robotics",
    });
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  // Everybody else with one of its pages open is on a path that now names
  // nothing: the organization's channel tells them where it went.
  it("tells the organization's channel where a rename moved it", async () => {
    wentBy("org-4k2j9x0q1z");
    fetchMock.mockResolvedValue(renamed("acme-robotics"));
    await renameOrganizationAction(ORGANIZATION, "Acme Robotics");
    expect(mockPublishEvent).toHaveBeenCalledTimes(1);
    expect(mockPublishEvent).toHaveBeenCalledWith(`bfev:organization:${ORGANIZATION}`, {
      type: "slug:moved",
      data: { organizationId: ORGANIZATION, from: "org-4k2j9x0q1z", to: "acme-robotics" },
    });
  });

  it("revalidates in place when it cannot tell whether the slug moved", async () => {
    vi.mocked(getServerContext).mockResolvedValue(null);
    fetchMock.mockResolvedValue(renamed("acme-robotics"));
    expect(await renameOrganizationAction(ORGANIZATION, "Acme Robotics")).toEqual({
      error: null,
    });
    expect(mockRevalidate).toHaveBeenCalledWith("/", "layout");
    expect(mockPublishEvent).not.toHaveBeenCalled();
  });

  it("tells nobody about a rename auth refused", async () => {
    wentBy("org-4k2j9x0q1z");
    fetchMock.mockResolvedValue(new Response(null, { status: 409 }));
    expect(await renameOrganizationAction(ORGANIZATION, "Acme Robotics")).toEqual({
      error: "problem",
    });
    expect(mockPublishEvent).not.toHaveBeenCalled();
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  // ⚠ The organization this page rendered has been renamed since, so its path
  // names nobody and `/me` answers another one at the click. Sent on, the name
  // typed into A's box would have renamed B — an owner of both would never
  // know which.
  it("refuses without asking auth when the request resolves another organization", async () => {
    standingIn("org_elsewhere");
    expect(await renameOrganizationAction(ORGANIZATION, "Acme Robotics")).toEqual({
      error: SWITCHED,
    });
    expect(fetchMock).not.toHaveBeenCalled();
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  it("refuses without asking auth when it cannot tell where the caller stands", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    expect(await renameOrganizationAction(ORGANIZATION, "Acme Robotics")).toEqual({
      error: "Couldn't resolve your organization right now. Try again in a moment.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // The form was posted from the path the organization went by before another
  // tab renamed it, and this one missed the move: its path names an
  // organization auth did not answer with, and a reload would answer "not
  // found", so "try again" would be the wrong advice.
  it("tells a tab that missed the move that its address is gone", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    vi.mocked(getServerContext).mockResolvedValue({
      organizationNotFound: true,
    } as never);
    const res = await renameOrganizationAction(ORGANIZATION, "Acme Robotics");
    expect(res.error).toMatch(/renamed, or you're no longer in it/);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
