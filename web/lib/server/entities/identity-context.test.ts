import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/env", () => ({ env: { SERVICE_SECRET: "s" } }));
// The real `activeOrganization`: which entry `/me` names is the thing under test.
vi.mock("./organization", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./organization")>()),
  getServerContext: vi.fn(),
}));
vi.mock("../session", () => ({ getServerSession: vi.fn() }));
const mockGet = vi.fn();
vi.mock("next/headers", () => ({
  cookies: async () => ({ get: mockGet }),
}));

import type { SessionData } from "@/lib/auth/session";

import { getServerContext, type ServerContext } from "./organization";
import { getServerSession } from "../session";
import {
  accountHeaders,
  identityContext,
  organizationHeaders,
  personHeaders,
  projectHeaders,
  sessionHeaders,
  type IdentityContext,
} from "./identity-context";

const SESSION = {
  userId: "user_me",
  email: "me@example.test",
  accessToken: "at_me",
} as SessionData;
const CTX: IdentityContext = {
  userId: "user_me",
  organizationId: "org_mine",
  role: "owner",
  accessToken: "at_me",
};

function me(activeOrganizationId: string): ServerContext {
  return {
    person: {
      userId: "user_me",
      email: "me@example.test",
      displayName: null,
      analyticsOptIn: false,
    },
    deletedOrganizations: [],
    organizations: [
      { organizationId: "org_mine", name: null, ownerEmail: "me@example.test", role: "owner" },
      { organizationId: "org_x", name: "Org X", ownerEmail: "x@example.test", role: "member" },
    ],
    activeOrganizationId,
    memberships: [],
    incomingInvites: [],
    projectOffers: [],
    flags: {},
  };
}

describe("identityContext", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue(SESSION);
    vi.mocked(getServerContext).mockResolvedValue(me("org_mine"));
    mockGet.mockReturnValue(undefined);
  });

  it("is null with no session", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null);
    expect(await identityContext()).toBeNull();
  });

  it("stands in the organization auth resolved, at the role it lists there, carrying the session's bearer", async () => {
    expect(await identityContext()).toEqual(CTX);
  });

  it("stands in an organization the caller only belongs to, as what they are there", async () => {
    vi.mocked(getServerContext).mockResolvedValue(me("org_x"));
    expect(await identityContext()).toEqual({ ...CTX, organizationId: "org_x", role: "member" });
  });

  // ⚠ The cookie goes to auth's `/me`, and auth answers with the
  // organization to act in; having a single source of truth ensures the page
  // and the server actions stay in lock-step.
  it("takes auth's answer over whatever the cookie asks for", async () => {
    for (const asked of ["org_x", "org_stranger"]) {
      mockGet.mockReturnValue({ value: asked });
      expect(await identityContext(), asked).toEqual(CTX);
    }
  });

  // ⚠ Without auth resolution, guessing an organization risks naming a tenant
  // that does not exist or does not match the caller's context.
  it("is null when auth could not say where the caller stands, rather than guessing", async () => {
    vi.mocked(getServerContext).mockResolvedValue(null);
    mockGet.mockReturnValue({ value: "org_x" });
    expect(await identityContext()).toBeNull();
  });

  it("is null when auth's answer names an organization its own list does not hold", async () => {
    vi.mocked(getServerContext).mockResolvedValue(me("org_gone"));
    expect(await identityContext()).toBeNull();
  });

  // Signed in, and in no organization — sign-ups closed. There is nothing to
  // act in, so nothing that acts in an organization may be offered one.
  it("is null for somebody auth places in no organization", async () => {
    vi.mocked(getServerContext).mockResolvedValue({
      ...me("org_mine"),
      organizations: [],
      activeOrganizationId: null,
    });
    expect(await identityContext()).toBeNull();
  });
});

// ⚠ Exact sets, not `objectContaining`. The bearer is the only statement of
// who is asking; a user id or a role spelt beside it would be sent, ignored by
// auth, and mistaken by the next reader for a mechanism.
describe("what a person lane receives", () => {
  function without(headers: Record<string, string>) {
    const { "x-request-id": requestId, ...rest } = headers;
    expect(requestId).toMatch(/^[0-9a-f-]{36}$/);
    return rest;
  }

  it("organizationHeaders relays the bearer and the organization, and names no project", () => {
    expect(without(organizationHeaders(CTX))).toEqual({
      authorization: "Bearer at_me",
      "x-service-secret": "s",
      "x-organization-id": "org_mine",
    });
  });

  it("projectHeaders adds the project and nothing else", () => {
    expect(without(projectHeaders(CTX, "project_1"))).toEqual({
      authorization: "Bearer at_me",
      "x-service-secret": "s",
      "x-organization-id": "org_mine",
      "x-project-id": "project_1",
    });
  });

  it("sessionHeaders builds the same set from a session, for the lanes that act on one directly", () => {
    expect(without(sessionHeaders(SESSION, "org_mine"))).toEqual({
      authorization: "Bearer at_me",
      "x-service-secret": "s",
      "x-organization-id": "org_mine",
    });
    expect(without(sessionHeaders(SESSION, "org_1", "project_1"))).toEqual({
      authorization: "Bearer at_me",
      "x-service-secret": "s",
      "x-organization-id": "org_1",
      "x-project-id": "project_1",
    });
  });

  // The account's own lanes — invitations, deletion — must reach somebody in
  // no organization, so they name none rather than a stale one.
  it("personHeaders relays the bearer and names no organization at all", () => {
    expect(without(personHeaders(SESSION))).toEqual({
      authorization: "Bearer at_me",
      "x-service-secret": "s",
    });
  });

  // ⚠ The account's settings are the person's wherever they stand, and auth
  // records the changes on the active organization's chain. Somebody in none
  // names none — and only they: auth refuses a missing header from anybody in
  // one, so "could not ask /me" must not pass for "in no organization".
  describe("accountHeaders", () => {
    beforeEach(() => {
      vi.mocked(getServerSession).mockResolvedValue(SESSION);
    });

    it("names the organization auth resolved", async () => {
      vi.mocked(getServerContext).mockResolvedValue(me("org_x"));
      expect(without((await accountHeaders())!)).toEqual({
        authorization: "Bearer at_me",
        "x-service-secret": "s",
        "x-organization-id": "org_x",
      });
    });

    it("names none for somebody in no organization", async () => {
      vi.mocked(getServerContext).mockResolvedValue({
        ...me("org_mine"),
        organizations: [],
        activeOrganizationId: null,
      });
      expect(without((await accountHeaders())!)).toEqual({
        authorization: "Bearer at_me",
        "x-service-secret": "s",
      });
    });

    it("is null when /me is unavailable, or there is no session", async () => {
      vi.mocked(getServerContext).mockResolvedValue(null);
      expect(await accountHeaders()).toBeNull();
      vi.mocked(getServerSession).mockResolvedValue(null);
      expect(await accountHeaders()).toBeNull();
    });
  });

  it("mints a fresh request id for every set", () => {
    expect(organizationHeaders(CTX)["x-request-id"]).not.toBe(
      organizationHeaders(CTX)["x-request-id"],
    );
  });

  // The context carries the role for what a page shows. It must not ride
  // along: every lane derives the role from its own tables.
  it("names the person through the bearer alone, and never a role", () => {
    for (const headers of [
      organizationHeaders(CTX),
      projectHeaders(CTX, "project_1"),
      sessionHeaders(SESSION, "org_mine"),
      personHeaders(SESSION),
    ]) {
      expect(headers).not.toHaveProperty("x-user-id");
      expect(headers).not.toHaveProperty("x-role");
      expect(headers).not.toHaveProperty("x-identity-signature");
      expect(headers).not.toHaveProperty("x-identity-expires");
    }
  });
});
