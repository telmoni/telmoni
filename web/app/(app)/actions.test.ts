import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/server/entities/organization", () => ({ getServerContext: vi.fn() }));
// `createProjectAction` now chooses which organization to NAME, so the rest of
// its collaborators have to be here too.
const mockFetch = vi.fn();
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: (...a: unknown[]) => mockFetch(...a),
  extractProblem: async () => ({ message: "problem" }),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: async () => null,
  sessionKey: () => "k",
}));
const mockOrganizationHeaders = vi.fn((ctx: { organizationId: string }) => ({
  authorization: "Bearer at_me",
  "x-organization-id": ctx.organizationId,
}));
vi.mock("@/lib/server/entities/identity-context", () => ({
  identityContext: async () => mockIdentity,
  organizationHeaders: (ctx: { organizationId: string }) => mockOrganizationHeaders(ctx),
}));
vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://auth.test" } }));
vi.mock("next/cache", () => ({ revalidatePath: vi.fn() }));
vi.mock("@/lib/logger", () => ({ logger: { warn: vi.fn() } }));
let mockIdentity: IdentityContext | null = null;

import type { IdentityContext } from "@/lib/server/entities/identity-context";
import {
  getServerContext,
  type OrganizationEntry,
  type ServerContext,
} from "@/lib/server/entities/organization";
import { getServerSession } from "@/lib/server/session";

import { createProjectAction } from "./actions";

// Every organization the caller is in, their own among them: an organization is
// an entry with a role in it, never an id that happens to be theirs.
function me(organizations: OrganizationEntry[], activeOrganizationId: string): ServerContext {
  return {
    person: { userId: "user_me", email: "me@example.test", analyticsOptIn: false },
    organizations,
    deletedOrganizations: [],
    activeOrganizationId,
    defaultOrganizationId: activeOrganizationId,
    organizationNotFound: false,
    memberships: [],
    incomingInvites: [],
    projectOffers: [],
    flags: {},
  };
}

const owned = (organizationId: string): OrganizationEntry => ({
  organizationId,
  slug: organizationId.replace("org_", ""),
  name: organizationId.replace("org_", ""),
  ownerEmail: "me@example.test",
  role: "owner",
});

const joined = (organizationId: string, role: "admin" | "member"): OrganizationEntry => ({
  organizationId,
  slug: organizationId.replace("org_", ""),
  name: organizationId.replace("org_", ""),
  ownerEmail: "someone@example.test",
  role,
});

// Standing in an organization the caller is only a MEMBER of, while owning one,
// administering another, and holding a seat in a third.
const STANDING = [
  owned("org_mine"),
  joined("org_seat", "member"),
  joined("org_admin", "admin"),
  joined("org_seat_other", "member"),
];

describe("createProjectAction — which organization the project lands in", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "user_me",
      email: "me@example.test",
    } as never);
    mockIdentity = {
      userId: "user_me",
      organizationId: "org_seat",
      role: "member",
      accessToken: "at_me",
    };
    vi.mocked(getServerContext).mockResolvedValue(me(STANDING, "org_seat"));
    mockFetch.mockResolvedValue({
      ok: true,
      json: async () => ({ id: "project_1", slug: "platform", name: "Platform", role: "owner" }),
    });
  });

  const namedOrganization = () =>
    mockOrganizationHeaders.mock.calls[0]?.[0]?.organizationId;

  it("names an organization the caller administers", async () => {
    const r = await createProjectAction("Platform", "org_admin");
    expect(r.error).toBeNull();
    expect(namedOrganization()).toBe("org_admin");
  });

  it("names an organization the caller owns", async () => {
    const r = await createProjectAction("Platform", "org_mine");
    expect(r.error).toBeNull();
    expect(namedOrganization()).toBe("org_mine");
  });

  // ⚠ **The console names `x-organization-id`.** Naming one the caller may
  // not act in would be asserting a claim on their behalf — so this must be
  // refused HERE, without a request, rather than left for auth to reject.
  it("refuses to name an organization the caller only holds a seat in", async () => {
    const r = await createProjectAction("Platform", "org_seat_other");
    expect(r.error).toMatch(/cannot create a project in that organization/i);
    expect(mockFetch).not.toHaveBeenCalled();
    expect(mockOrganizationHeaders).not.toHaveBeenCalled();
  });

  it("refuses to name an organization the caller is not in at all", async () => {
    const r = await createProjectAction("Platform", "org_stranger");
    expect(r.error).toMatch(/cannot create a project in that organization/i);
    expect(mockFetch).not.toHaveBeenCalled();
  });

  // ⚠ Ownership is the `owner` entry on the organization, and an id that
  // names no organization is refused like any stranger's.
  it("refuses the caller's user id, which names no organization", async () => {
    const r = await createProjectAction("Platform", "user_me");
    expect(r.error).toMatch(/cannot create a project in that organization/i);
    expect(mockFetch).not.toHaveBeenCalled();
    expect(mockOrganizationHeaders).not.toHaveBeenCalled();
  });

  // Naming the organization you are already standing in is not a widening —
  // auth resolved it from the caller's own organizations to get here. So it
  // passes through and `can_create_projects` is what refuses a member, on auth's
  // side, where the roster actually lives.
  it("passes the active organization through and leaves the role to auth", async () => {
    const r = await createProjectAction("Platform", "org_seat");
    expect(r.error).toBeNull();
    expect(namedOrganization()).toBe("org_seat");
    expect(mockFetch).toHaveBeenCalled();
  });

  it("falls back to the active organization when none is named", async () => {
    const r = await createProjectAction("Platform");
    expect(r.error).toBeNull();
    expect(namedOrganization()).toBe("org_seat");
  });
});

// ⚠ **The project is created over there; the console has to go over there too.**
// The path names the organization, so the dialog follows by opening the
// project's own path — which it can only spell with the slug auth gave it.
describe("createProjectAction — what the dialog is handed to follow the project", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "user_me",
      email: "me@example.test",
    } as never);
    mockIdentity = {
      userId: "user_me",
      organizationId: "org_seat",
      role: "member",
      accessToken: "at_me",
    };
    vi.mocked(getServerContext).mockResolvedValue(me(STANDING, "org_seat"));
    mockFetch.mockResolvedValue({
      ok: true,
      json: async () => ({ id: "project_1", slug: "platform-2", name: "Platform", role: "owner" }),
    });
  });

  // Auth picks the slug: a name another project already reads as takes the
  // next number, and only auth knows which.
  it("answers the project with the slug auth minted for it", async () => {
    const r = await createProjectAction("Platform", "org_admin");
    expect(r).toEqual({
      error: null,
      project: { id: "project_1", slug: "platform-2", name: "Platform", role: "owner" },
    });
  });

  it("answers no project to follow when auth's answer cannot be read", async () => {
    mockFetch.mockResolvedValue({
      ok: true,
      json: async () => ({ id: "project_1", name: "Platform", role: "owner" }),
    });
    const r = await createProjectAction("Platform", "org_admin");
    expect(r).toEqual({ error: null });
  });

  it("answers no project when it was not created", async () => {
    mockFetch.mockResolvedValue({ ok: false });
    const r = await createProjectAction("Platform", "org_admin");
    expect(r.error).toBe("problem");
    expect(r.project).toBeUndefined();
  });
});
