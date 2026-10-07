import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/server/entities/organization", () => ({ getServerContext: vi.fn() }));
// `createProjectAction` now chooses which organization to NAME, so the rest of
// its collaborators have to be here too.
const mockFetch = vi.fn();
const mockExtractProblem = vi.fn<
  (res: unknown) => Promise<{ message: string; problem: Record<string, unknown> | null }>
>(async () => ({ message: "problem", problem: null }));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: (...a: unknown[]) => mockFetch(...a),
  extractProblem: (res: unknown) => mockExtractProblem(res),
}));
const mockRateLimit = vi.fn<(key: string, opts: unknown) => Promise<unknown>>(async () => null);
const mockSessionKey = vi.fn<(session: unknown, bucket: string) => string>(() => "k");
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: (key: string, opts: unknown) => mockRateLimit(key, opts),
  sessionKey: (session: unknown, bucket: string) => mockSessionKey(session, bucket),
}));
const mockOrganizationHeaders = vi.fn((ctx: { organizationId: string }) => ({
  authorization: "Bearer at_me",
  "x-organization-id": ctx.organizationId,
}));
const mockPersonHeaders = vi.fn<(session: unknown) => Record<string, string>>(() => ({
  authorization: "Bearer at_me",
}));
vi.mock("@/lib/server/entities/identity-context", () => ({
  identityContext: async () => mockIdentity,
  organizationHeaders: (ctx: { organizationId: string }) => mockOrganizationHeaders(ctx),
  personHeaders: (session: unknown) => mockPersonHeaders(session),
}));
const mockPublishEvent = vi.fn<(channel: string, event: unknown) => Promise<boolean>>(
  async () => true,
);
vi.mock("@/lib/events/publisher", () => ({
  publishEvent: (channel: string, event: unknown) => mockPublishEvent(channel, event),
  userChannel: (email: string) => `user:${email}`,
}));
vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://auth.test" } }));
vi.mock("next/cache", () => ({ revalidatePath: vi.fn() }));
vi.mock("@/lib/logger", () => ({ logger: { warn: vi.fn() } }));
let mockIdentity: IdentityContext | null = null;

import { revalidatePath } from "next/cache";

import type { IdentityContext } from "@/lib/server/entities/identity-context";
import {
  getServerContext,
  type OrganizationEntry,
  type ServerContext,
} from "@/lib/server/entities/organization";
import { getServerSession } from "@/lib/server/session";

import { createOrganizationAction, createProjectAction } from "./actions";

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

describe("createOrganizationAction", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({
      userId: "user_me",
      email: "me@example.test",
    } as never);
    mockFetch.mockResolvedValue({
      ok: true,
      status: 201,
      json: async () => ({ id: "org_new", slug: "acme", name: "Acme" }),
    });
  });

  const sent = () =>
    mockFetch.mock.calls[0] as
      | [string, { method: string; headers: Record<string, string>; body: string }]
      | undefined;

  // ⚠ **It names no organization.** The new one has no id until auth mints
  // it, and naming the one on screen would assert a claim the act does not
  // need, on a chain it does not touch.
  it("founds it with the person's own headers, naming no organization, and answers its Overview", async () => {
    const r = await createOrganizationAction("  Acme  ", "");
    expect(r).toEqual({ error: null, href: "/acme" });
    const [url, init] = sent()!;
    expect(url).toBe("http://auth.test/internal/organizations");
    expect(init.method).toBe("POST");
    expect(init.headers.authorization).toBe("Bearer at_me");
    expect(init.headers["content-type"]).toBe("application/json");
    expect(init.headers).not.toHaveProperty("x-organization-id");
    expect(mockOrganizationHeaders).not.toHaveBeenCalled();
    expect(JSON.parse(init.body)).toEqual({ name: "Acme" });
    expect(revalidatePath).toHaveBeenCalledWith("/", "layout");
  });

  it("sends a URL when one is given, and leaves a blank one to auth to derive", async () => {
    await createOrganizationAction("Acme", " acme-labs ");
    expect(JSON.parse(sent()![1].body)).toEqual({ name: "Acme", slug: "acme-labs" });
  });

  it("is throttled per person, as project creation is", async () => {
    await createOrganizationAction("Acme", "");
    expect(mockSessionKey).toHaveBeenCalledWith(
      expect.objectContaining({ userId: "user_me" }),
      "organization:create",
    );
    expect(mockRateLimit).toHaveBeenCalledWith("k", { limit: 10, windowMs: 60_000 });

    mockFetch.mockClear();
    mockRateLimit.mockResolvedValueOnce({ status: 429 });
    const r = await createOrganizationAction("Acme", "");
    expect(r.error).toMatch(/too many requests/i);
    expect(mockFetch).not.toHaveBeenCalled();
  });

  // The person's other tabs learn of it as they learn of an organization an
  // ownership transfer hands them, on the person's own channel: no tab is
  // subscribed to the new organization's yet.
  it("tells the person's other tabs", async () => {
    await createOrganizationAction("Acme", "");
    expect(mockPublishEvent).toHaveBeenCalledWith("user:me@example.test", {
      type: "ownership:changed",
      data: { organizationId: "org_new" },
    });
  });

  it("refuses a name it cannot take under Name, without a request", async () => {
    for (const name of ["", "   ", "x".repeat(81)]) {
      const r = await createOrganizationAction(name, "");
      expect(r.field, name).toBe("name");
      expect(r.error, name).toBeTruthy();
    }
    expect(mockFetch).not.toHaveBeenCalled();
  });

  // An astral letter is two UTF-16 units: counted by `length`, eighty of
  // them would read as 160 and be refused.
  it("counts a name in characters, as auth does", async () => {
    const r = await createOrganizationAction("\u{1D49C}".repeat(80), "");
    expect(r.error).toBeNull();
    expect(mockFetch).toHaveBeenCalled();

    mockFetch.mockClear();
    const over = await createOrganizationAction("\u{1D49C}".repeat(81), "");
    expect(over.field).toBe("name");
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it("refuses a URL of the wrong shape or a console word under URL, without a request", async () => {
    for (const slug of ["Acme", "acme--labs", "a".repeat(49), "settings", "console"]) {
      const r = await createOrganizationAction("Acme", slug);
      expect(r.field, slug).toBe("slug");
      expect(r.error, slug).toBeTruthy();
    }
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it("shows a URL another organization holds under URL", async () => {
    mockFetch.mockResolvedValue({ ok: false, status: 409 });
    mockExtractProblem.mockResolvedValueOnce({
      message: "conflict: another organization already has that URL",
      problem: { type: "/errors/auth/conflict", title: "conflict", status: 409 },
    });
    const r = await createOrganizationAction("Acme", "acme");
    expect(r).toEqual({
      error: "conflict: another organization already has that URL",
      field: "slug",
    });
    expect(mockPublishEvent).not.toHaveBeenCalled();
    expect(revalidatePath).not.toHaveBeenCalled();
  });

  // A zero-width space passes for a name here, and auth cleans it to nothing.
  it("shows auth's refusal of the name under Name", async () => {
    mockFetch.mockResolvedValue({ ok: false, status: 400 });
    mockExtractProblem.mockResolvedValueOnce({
      message: "bad request: give the organization a name",
      problem: { type: "/errors/auth/bad-request", title: "bad request", status: 400 },
    });
    const r = await createOrganizationAction("\u200B", "");
    expect(r).toEqual({ error: "bad request: give the organization a name", field: "name" });
  });

  it("says sign-ups are closed in the flag's own sentence", async () => {
    mockFetch.mockResolvedValue({ ok: false, status: 503 });
    mockExtractProblem.mockResolvedValueOnce({
      message: "feature switched off: Sign-ups are closed right now.",
      problem: {
        type: "/errors/tenant/feature-off",
        title: "feature switched off",
        status: 503,
        detail: "Sign-ups are closed right now.",
        flag: "signup",
      },
    });
    const r = await createOrganizationAction("Acme", "");
    expect(r).toEqual({ error: "Sign-ups are closed right now." });
  });

  // With no URL asked for, a conflict is about nothing the person typed.
  it("puts a conflict under no field when no URL was asked for", async () => {
    mockFetch.mockResolvedValue({ ok: false, status: 409 });
    mockExtractProblem.mockResolvedValueOnce({
      message: "conflict: That already exists",
      problem: { type: "/errors/auth/conflict", title: "conflict", status: 409 },
    });
    const r = await createOrganizationAction("Acme", "");
    expect(r).toEqual({ error: "conflict: That already exists" });
  });

  it("asks for a sign-in when auth no longer takes the session", async () => {
    mockFetch.mockResolvedValue({ ok: false, status: 401 });
    const r = await createOrganizationAction("Acme", "");
    expect(r).toEqual({ error: "Your session expired — sign in again." });
    expect(mockExtractProblem).not.toHaveBeenCalled();
  });

  it("answers a refusal about neither field with no field", async () => {
    mockFetch.mockResolvedValue({ ok: false, status: 403 });
    mockExtractProblem.mockResolvedValueOnce({
      message: "forbidden: account deletion in progress",
      problem: { type: "/errors/authz/forbidden", title: "forbidden", status: 403 },
    });
    const r = await createOrganizationAction("Acme", "");
    expect(r).toEqual({ error: "forbidden: account deletion in progress" });
  });

  it("says so when auth cannot be reached", async () => {
    mockFetch.mockResolvedValue(null);
    const r = await createOrganizationAction("Acme", "");
    expect(r.error).toMatch(/unavailable/i);
    expect(r.field).toBeUndefined();
  });

  it("asks for a sign-in when the session is gone, without a request", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null as never);
    const r = await createOrganizationAction("Acme", "");
    expect(r.error).toMatch(/session expired/i);
    expect(mockFetch).not.toHaveBeenCalled();
  });

  // Auth spells the new address. An answer that names no slug, or one that is
  // not a slug's shape, is not followed — `//host` would leave the console.
  it("follows no address auth's answer does not give as a slug", async () => {
    for (const body of [
      { id: "org_new", name: "Acme" },
      { id: "org_new", slug: "/evil.example", name: "Acme" },
    ]) {
      mockFetch.mockResolvedValueOnce({ ok: true, status: 201, json: async () => body });
      const r = await createOrganizationAction("Acme", "");
      expect(r).toEqual({ error: null });
    }
    expect(mockPublishEvent).not.toHaveBeenCalled();
    expect(revalidatePath).toHaveBeenCalledWith("/", "layout");
  });
});
