import { beforeEach, describe, expect, it, vi } from "vitest";

const { fetchWithTimeout, identityContext, getServerContext } = vi.hoisted(() => ({
  fetchWithTimeout: vi.fn(),
  identityContext: vi.fn(),
  getServerContext: vi.fn(),
}));

vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://auth" } }));
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("./identity-context", () => ({
  identityContext,
  organizationHeaders: () => ({ authorization: "Bearer at_1", "x-organization-id": "org_1" }),
}));
vi.mock("./organization", () => ({ getServerContext }));
vi.mock("next/navigation", () => ({
  notFound: () => {
    throw new Error("NOT_FOUND");
  },
}));
import { fetchProjectAnywhere, fetchProjectBySlug } from "./projects";

const WEB = { id: "project_1", slug: "web", name: "Web", role: "owner" };
const API = { id: "project_2", slug: "api", name: "API", role: "member" };

function listing(projects: unknown[]) {
  fetchWithTimeout.mockResolvedValue({ ok: true, json: async () => ({ projects }) });
}

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
  getServerContext.mockResolvedValue({ organizationNotFound: false });
  listing([WEB, API]);
});

describe("fetchProjectBySlug", () => {
  it("finds the project the path names in the organization's listing", async () => {
    expect(await fetchProjectBySlug("api")).toMatchObject({ id: "project_2", slug: "api" });
  });

  // A rename, a delete or a transfer since the link was drawn: the listing
  // answered, and no project in it goes by that slug.
  it("is not found for a slug no project in the organization goes by", async () => {
    await expect(fetchProjectBySlug("gone")).rejects.toThrow("NOT_FOUND");
  });

  // ⚠ Slugs are unique within an organization, not across them: every one
  // starts with a `default-project`. Auth answered another organization than
  // the path names, and its project of the same slug is a different project.
  it("is not found when the path names an organization auth did not answer with", async () => {
    getServerContext.mockResolvedValue({ organizationNotFound: true });
    await expect(fetchProjectBySlug("web")).rejects.toThrow("NOT_FOUND");
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  // An outage is not "gone": the page says the service is unavailable.
  it("answers nothing, rather than not found, when the listing cannot be read", async () => {
    fetchWithTimeout.mockResolvedValue({ ok: false });
    expect(await fetchProjectBySlug("web")).toBeNull();

    identityContext.mockResolvedValue(null);
    expect(await fetchProjectBySlug("web")).toBeNull();
  });

  it("never reads a project's id as its slug", async () => {
    await expect(fetchProjectBySlug("project_1")).rejects.toThrow("NOT_FOUND");
  });
});

describe("fetchProjectAnywhere", () => {
  const elsewhere = {
    ...WEB,
    organizationId: "org_home",
    organizationSlug: "acme",
    organizationName: "Acme",
    organizationOwnerEmail: null,
  };

  it("finds a project by id in whichever organization holds it", async () => {
    listing([elsewhere]);
    expect(await fetchProjectAnywhere("project_1")).toMatchObject({
      id: "project_1",
      organizationId: "org_home",
      organizationSlug: "acme",
    });
    expect(fetchWithTimeout.mock.calls[0]?.[0]).toBe("http://auth/internal/projects/everywhere");
  });

  it("answers nothing for a project the person cannot open, or a slug", async () => {
    listing([elsewhere]);
    expect(await fetchProjectAnywhere("project_other")).toBeNull();
    expect(await fetchProjectAnywhere("web")).toBeNull();
  });
});
