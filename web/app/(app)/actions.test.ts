import { beforeEach, describe, expect, it, vi } from "vitest";

const mockSet = vi.fn();
vi.mock("next/headers", () => ({
  cookies: async () => ({ set: mockSet }),
}));
const mockRedirect = vi.fn();
vi.mock("next/navigation", () => ({
  redirect: (url: string) => {
    mockRedirect(url);
    throw new Error(`REDIRECT:${url}`);
  },
}));
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

import { createProjectAction, switchActiveOrganizationAction } from "./actions";

// Every organization the caller is in, their own among them: an organization is
// an entry with a role in it, never an id that happens to be theirs.
function me(organizations: OrganizationEntry[], activeOrganizationId: string): ServerContext {
  return {
    person: { userId: "user_me", email: "me@example.test", analyticsOptIn: false },
    organizations,
    deletedOrganizations: [],
    activeOrganizationId,
    memberships: [],
    incomingInvites: [],
    projectOffers: [],
    flags: {},
  };
}

const owned = (organizationId: string): OrganizationEntry => ({
  organizationId,
  ownerEmail: "me@example.test",
  role: "owner",
});

const joined = (organizationId: string, role: "admin" | "member"): OrganizationEntry => ({
  organizationId,
  ownerEmail: "someone@example.test",
  role,
});

describe("switchActiveOrganizationAction", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "user_me",
      email: "me@example.test",
    } as never);
    vi.mocked(getServerContext).mockResolvedValue(
      me([owned("org_mine"), joined("org_x", "member")], "org_mine"),
    );
  });

  it("sends a signed-out caller to login and sets nothing", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null);
    await expect(switchActiveOrganizationAction("org_x")).rejects.toThrow("REDIRECT:/auth/login");
    expect(mockSet).not.toHaveBeenCalled();
  });

  it("sets nothing when auth could not say which organizations the caller is on", async () => {
    vi.mocked(getServerContext).mockResolvedValue(null);
    await expect(switchActiveOrganizationAction("org_x")).rejects.toThrow("REDIRECT:/console");
    expect(mockSet).not.toHaveBeenCalled();
  });

  // ⚠ These three asserted `/console`, which resolves the caller's project
  // listing and redirects to the FIRST project — so picking an organization out
  // of the switcher dropped you inside one of its projects and you had to reopen
  // the switcher to reach the organization you had just clicked. The error
  // paths above still land on `/console`; that is a safe landing, not a
  // destination somebody asked for.
  it("switches to an organization the caller is on the roster of", async () => {
    await expect(switchActiveOrganizationAction("org_x")).rejects.toThrow(
      "REDIRECT:/org_x",
    );
    expect(mockSet).toHaveBeenCalledWith(
      "telmoni-active-organization",
      "org_x",
      expect.objectContaining({ httpOnly: true, sameSite: "lax", path: "/" }),
    );
  });

  it("switches back to an organization the caller owns", async () => {
    await expect(switchActiveOrganizationAction("org_mine")).rejects.toThrow(
      "REDIRECT:/org_mine",
    );
    expect(mockSet).toHaveBeenCalledWith("telmoni-active-organization", "org_mine", expect.anything());
  });

  // ⚠ The caller's user id names no organization, and a cookie holding it
  // would be ignored by auth — a switch must name an actual organization.
  it("refuses the caller's own user id, which names no organization", async () => {
    await expect(switchActiveOrganizationAction("user_me")).rejects.toThrow(
      "Unauthorized organization switch",
    );
    expect(mockSet).not.toHaveBeenCalled();
    expect(mockRedirect).not.toHaveBeenCalled();
  });

  it("redirects to canonical slug when organization has a slug", async () => {
    const orgWithSlug: OrganizationEntry = {
      organizationId: "org_slugged",
      slug: "acme-corp",
      ownerEmail: "me@example.test",
      role: "owner",
    };
    vi.mocked(getServerContext).mockResolvedValue(
      me([orgWithSlug], "org_slugged"),
    );

    await expect(
      switchActiveOrganizationAction("acme-corp", "my-web-app"),
    ).rejects.toThrow("REDIRECT:/acme-corp/my-web-app");
    expect(mockSet).toHaveBeenCalledWith(
      "telmoni-active-organization",
      "org_slugged",
      expect.anything(),
    );
  });

  it("lands on the project named, in the organization switched to", async () => {
    await expect(
      switchActiveOrganizationAction("org_x", "project_7bQx2mNv9BcK4dLp"),
    ).rejects.toThrow("REDIRECT:/org_x/project_7bQx2mNv9BcK4dLp");
    expect(mockSet).toHaveBeenCalledWith("telmoni-active-organization", "org_x", expect.anything());
  });

  it.each([
    "//evil.example",
    "organization/settings",
    "project_short",
    "/project_7bQx2mNv9BcK4dLp",
    "project_7bQx2mNv9BcK4dLp/../../auth/logout",
  ])("ignores a destination that is not a project id: %s", async (bad) => {
    await expect(switchActiveOrganizationAction("org_x", bad)).rejects.toThrow(
      "REDIRECT:/org_x",
    );
    expect(mockSet).toHaveBeenCalledWith("telmoni-active-organization", "org_x", expect.anything());
  });

  // The two rows in the switcher are two destinations, and the difference is
  // the whole bug: one takes you to the organization, the other to a project
  // inside it. Pinned side by side so neither can drift onto the other.
  it("takes an organization row to the organization and a project row to the project", async () => {
    await expect(switchActiveOrganizationAction("org_x")).rejects.toThrow(
      "REDIRECT:/org_x",
    );
    await expect(
      switchActiveOrganizationAction("org_x", "project_7bQx2mNv9BcK4dLp"),
    ).rejects.toThrow("REDIRECT:/org_x/project_7bQx2mNv9BcK4dLp");
  });

  it("refuses an organization the caller is not on, and sets nothing", async () => {
    await expect(switchActiveOrganizationAction("org_stranger")).rejects.toThrow(
      "Unauthorized organization switch",
    );
    expect(mockSet).not.toHaveBeenCalled();
    expect(mockRedirect).not.toHaveBeenCalled();
  });
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
      json: async () => ({ id: "project_1", name: "Platform", role: "owner" }),
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
// The dialog pushes `/${project.id}` on success, and `[projectId]/layout.tsx`
// resolves the project listing for whatever the active-organization cookie says —
// so without this the caller lands on "Not found" holding a project that exists.
describe("createProjectAction — where the console is left standing", () => {
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
      json: async () => ({ id: "project_1", name: "Platform", role: "owner" }),
    });
  });

  const switchedTo = () =>
    mockSet.mock.calls.find(
      ([name]) => name === "telmoni-active-organization",
    )?.[1];

  it("follows the project into an organization the caller owns", async () => {
    await createProjectAction("Platform", "org_mine");
    expect(switchedTo()).toBe("org_mine");
  });

  it("follows the project into an organization the caller administers", async () => {
    await createProjectAction("Platform", "org_admin");
    expect(switchedTo()).toBe("org_admin");
  });

  it("leaves the cookie alone when the project lands where they already are", async () => {
    await createProjectAction("Platform", "org_seat");
    expect(mockSet).not.toHaveBeenCalled();
  });

  it("does not switch to an organization it refused to sign for", async () => {
    const r = await createProjectAction("Platform", "org_seat_other");
    expect(r.error).toMatch(/cannot create a project/i);
    expect(mockSet).not.toHaveBeenCalled();
  });

  it("does not switch when the project was not created", async () => {
    mockFetch.mockResolvedValue({ ok: false });
    const r = await createProjectAction("Platform", "org_admin");
    expect(r.error).toBe("problem");
    expect(mockSet).not.toHaveBeenCalled();
  });
});
