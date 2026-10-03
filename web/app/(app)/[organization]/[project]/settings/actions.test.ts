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
// The real `projectHeaders`: what reaches auth is the assertion under test.
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext: vi.fn(),
}));
// `/me`'s answer for the request: the organization the path names, whose slug
// the announcement of a moved project is spelled with.
vi.mock("@/lib/server/entities/organization", () => ({
  getServerContext: vi.fn(),
  activeOrganization: (ctx: {
    activeOrganizationId: string;
    organizations: { organizationId: string }[];
  }) => ctx.organizations.find((o) => o.organizationId === ctx.activeOrganizationId) ?? null,
}));
// The organization's listing, which is where the action reads the slug the
// project went by before the rename.
vi.mock("@/lib/server/entities/projects", () => ({
  fetchProject: vi.fn(),
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
import { fetchProject } from "@/lib/server/entities/projects";
import { getServerSession } from "@/lib/server/session";

import { updateProjectNameAction } from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

const ORGANIZATION = "org_acme";
const PROJECT = "project_1";

// Auth's answer to a rename: the name as stored, and the slug it goes by now.
const renamed = (slug: string) =>
  new Response(JSON.stringify({ status: "updated", name: "Marketing Site", slug }), {
    status: 200,
  });

function wentBy(slug: string) {
  vi.mocked(fetchProject).mockResolvedValue({ id: PROJECT, slug, name: "Web", role: null });
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "owner@example.test",
    accessToken: "at_1",
  } as never);
  vi.mocked(identityContext).mockResolvedValue({
    userId: "user_1",
    organizationId: ORGANIZATION,
    role: "owner",
    accessToken: "at_1",
  });
  vi.mocked(getServerContext).mockResolvedValue({
    activeOrganizationId: ORGANIZATION,
    organizations: [{ organizationId: ORGANIZATION, slug: "acme" }],
  } as never);
  wentBy("marketing-site");
});

describe("updateProjectNameAction", () => {
  it("renames the project by its id, in the organization the request acts in", async () => {
    fetchMock.mockResolvedValue(renamed("marketing-site"));
    expect(await updateProjectNameAction(PROJECT, "  Marketing Site  ")).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://auth.test/internal/projects/${PROJECT}`,
      expect.objectContaining({
        method: "PATCH",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": ORGANIZATION,
          "x-project-id": PROJECT,
          "content-type": "application/json",
        }),
        body: JSON.stringify({ name: "Marketing Site" }),
      }),
    );
    expect(mockRevalidate).toHaveBeenCalledWith("/(app)/[organization]/[project]", "layout");
    // Its paths are where they were: nobody has anywhere to follow it to.
    expect(mockPublishEvent).not.toHaveBeenCalled();
  });

  // ⚠ A rename moves the project's slug with its name, out from under the
  // path this was posted from. The page follows it; rendering the old path
  // again first, as a revalidation would, answers "not found".
  it("says where a rename moved the project, and revalidates nothing", async () => {
    wentBy("web");
    fetchMock.mockResolvedValue(renamed("marketing-site"));
    expect(await updateProjectNameAction(PROJECT, "Marketing Site")).toEqual({
      error: null,
      movedTo: "marketing-site",
    });
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  // Everybody else with one of its pages open is on a path that now names
  // nothing. The announcement names the organization by the slug their path
  // is spelled with: two organizations may each hold a project called `web`.
  it("tells the organization's channel where a rename moved the project", async () => {
    wentBy("web");
    fetchMock.mockResolvedValue(renamed("marketing-site"));
    await updateProjectNameAction(PROJECT, "Marketing Site");
    expect(mockPublishEvent).toHaveBeenCalledTimes(1);
    expect(mockPublishEvent).toHaveBeenCalledWith(`bfev:organization:${ORGANIZATION}`, {
      type: "slug:moved",
      data: {
        organizationId: ORGANIZATION,
        organization: "acme",
        projectId: PROJECT,
        from: "web",
        to: "marketing-site",
      },
    });
  });

  it("revalidates in place when it cannot tell whether the slug moved", async () => {
    vi.mocked(fetchProject).mockResolvedValue(null);
    fetchMock.mockResolvedValue(renamed("marketing-site"));
    expect(await updateProjectNameAction(PROJECT, "Marketing Site")).toEqual({ error: null });
    expect(mockRevalidate).toHaveBeenCalledWith("/(app)/[organization]/[project]", "layout");
    expect(mockPublishEvent).not.toHaveBeenCalled();
  });

  it("surfaces auth's refusal, and tells nobody", async () => {
    wentBy("web");
    fetchMock.mockResolvedValue(new Response(null, { status: 409 }));
    expect(await updateProjectNameAction(PROJECT, "Marketing Site")).toEqual({ error: "problem" });
    expect(mockPublishEvent).not.toHaveBeenCalled();
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  it("refuses an empty or overlong name without asking auth", async () => {
    expect(await updateProjectNameAction(PROJECT, "   ")).toEqual({
      error: "Project name cannot be empty.",
    });
    expect(await updateProjectNameAction(PROJECT, "x".repeat(101))).toEqual({
      error: "Project name must be 100 characters or fewer.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("refuses without asking auth when it cannot tell where the caller stands", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    expect(await updateProjectNameAction(PROJECT, "Marketing Site")).toEqual({
      error: "Couldn't resolve your organization right now. Try again in a moment.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // Posted from a tab whose organization changed its URL, or let the person
  // go, after the page rendered: its path names an organization auth did not
  // answer with, and a reload would answer "not found".
  it("tells a tab that missed a move that its address is gone", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    vi.mocked(getServerContext).mockResolvedValue({
      organizationNotFound: true,
    } as Awaited<ReturnType<typeof getServerContext>>);
    const res = await updateProjectNameAction(PROJECT, "Marketing Site");
    expect(res.error).toMatch(/URL was changed, or you're no longer in it/);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
