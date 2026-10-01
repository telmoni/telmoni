import { beforeEach, describe, expect, it, vi } from "vitest";

const { fetchWithTimeout, cookieNamed } = vi.hoisted(() => ({
  fetchWithTimeout: vi.fn(),
  cookieNamed: vi.fn(),
}));

vi.mock("@/lib/env", () => ({
  env: { SERVICE_SECRET: "s", SERVER_URL: "http://auth" },
}));
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("next/headers", () => ({
  headers: async () => new Headers({ "user-agent": "Mozilla/5.0 (test)" }),
  cookies: async () => ({ get: cookieNamed }),
}));
vi.mock("../session", () => ({
  getServerSession: async () => ({
    userId: "user_1",
    email: "ada@example.test",
    firstName: "Ada",
    lastName: "Lovelace",
    accessToken: "at_1",
  }),
}));

import { activeOrganization, getServerContext, type ServerContext } from "./organization";

function meAnswers(body: Record<string, unknown>) {
  fetchWithTimeout.mockResolvedValue({ ok: true, json: async () => body });
}

const PERSON = {
  userId: "user_1",
  email: "ada@example.test",
  displayName: "Ada Lovelace",
  analyticsOptIn: false,
};

const OWNED = {
  organizationId: "org_1",
  name: null,
  ownerEmail: "ada@example.test",
  ownerDisplayName: "Ada Lovelace",
  role: "owner",
};

const JOINED = {
  organizationId: "org_2",
  name: "Analytical Engines",
  ownerEmail: "charles@example.test",
  ownerDisplayName: "Charles Babbage",
  role: "admin",
  ownershipOfferExpiresAt: "2026-09-30T00:00:00Z",
};

function me(over: Record<string, unknown> = {}) {
  return {
    person: PERSON,
    organizations: [OWNED, JOINED],
    activeOrganizationId: "org_1",
    ...over,
  };
}

function sentBody() {
  return JSON.parse(fetchWithTimeout.mock.calls[0][1].body as string);
}

function sentHeaders(): Record<string, string> {
  return fetchWithTimeout.mock.calls[0][1].headers;
}

beforeEach(() => {
  fetchWithTimeout.mockReset();
  cookieNamed.mockReset();
});

describe("what getServerContext asks /me to resolve", () => {
  // The bearer names the person and auth describes them from what the provider
  // told it at the exchange. The body carries only the browser it lists the session
  // under — never a user id, an address or a name auth would take on trust
  // (it refuses all three).
  it("names the person through the bearer and sends only the browser", async () => {
    meAnswers(me({ memberships: [], flags: {} }));
    await getServerContext();
    expect(sentHeaders().authorization).toBe("Bearer at_1");
    expect(sentHeaders()["x-service-secret"]).toBe("s");
    expect(sentHeaders()["x-request-id"]).toBeTruthy();
    expect(sentBody()).toEqual({ userAgent: "Mozilla/5.0 (test)" });
  });

  // A request, not a claim: auth answers with that organization only when the
  // person is in it, and says which one it chose either way.
  it("asks for the organization the active-organization cookie names", async () => {
    cookieNamed.mockImplementation((name: string) =>
      name === "telmoni-active-organization" ? { value: "org_2" } : undefined,
    );
    meAnswers(me());
    await getServerContext();
    expect(sentHeaders()["x-organization-id"]).toBe("org_2");
  });

  // ⚠ Without a cookie the console names no organization. Auth picks one they
  // own, else one they belong to.
  it("names no organization when no cookie asks for one", async () => {
    meAnswers(me());
    await getServerContext();
    expect(sentHeaders()).not.toHaveProperty("x-organization-id");
    expect(Object.values(sentHeaders())).not.toContain("user_1");
  });

  it("returns the person, every organization they are in, the one auth resolved, and the flags", async () => {
    meAnswers(
      me({
        activeOrganizationId: "org_2",
        memberships: [{ projectId: "project_1", organizationId: "org_2", role: "admin" }],
        flags: { api_tokens: false },
      }),
    );
    const ctx = await getServerContext();
    expect(ctx?.person).toEqual(PERSON);
    expect(ctx?.organizations).toEqual([OWNED, JOINED]);
    expect(ctx?.activeOrganizationId).toBe("org_2");
    expect(ctx?.memberships).toEqual([
      { projectId: "project_1", organizationId: "org_2", role: "admin" },
    ]);
    expect(ctx?.flags).toEqual({ api_tokens: false });
  });

  // The organizations the person owns that are on their way out ride beside
  // the live ones, never among them; an answer without the list reads as
  // none, since an older auth simply had nothing to offer back.
  it("reads the organizations being deleted apart from the live ones, and none as none", async () => {
    const closed = {
      organizationId: "org_closed",
      name: "Closed Co",
      deletionRequestedAt: "2026-09-23T10:00:00Z",
      eraseAfter: "2026-10-07T10:00:00Z",
      restorable: true,
    };
    meAnswers(me({ deletedOrganizations: [closed] }));
    const ctx = await getServerContext();
    expect(ctx?.deletedOrganizations).toEqual([closed]);
    expect(ctx?.organizations).toEqual([OWNED, JOINED]);

    meAnswers(me());
    expect((await getServerContext())?.deletedOrganizations).toEqual([]);
  });

  // An offer of a project is auth's row, listed beside the organizations and
  // labelled by the organization it would leave; an answer that carries none
  // reads as none, not as a shape we cannot read.
  it("reads the projects offered to the person, and none as none", async () => {
    const offered = {
      projectId: "project_1",
      name: "Payments",
      organizationId: "org_2",
      organizationName: "Analytical Engines",
      ownerEmail: "charles@example.test",
      ownerDisplayName: "Charles Babbage",
      expiresAt: "2026-10-05T00:00:00Z",
    };
    meAnswers(me({ projectOffers: [offered] }));
    expect((await getServerContext())?.projectOffers).toEqual([offered]);

    meAnswers(me());
    expect((await getServerContext())?.projectOffers).toEqual([]);
  });

  it("floors an unknown role spelling to null rather than passing it through", async () => {
    meAnswers(
      me({
        memberships: [{ projectId: "project_1", organizationId: "org_2", role: "superuser" }],
      }),
    );
    const ctx = await getServerContext();
    expect(ctx?.memberships[0]?.role).toBeNull();
  });

  it("is null when auth refuses, and null is not an empty organization", async () => {
    fetchWithTimeout.mockResolvedValue({ ok: false, status: 503 });
    expect(await getServerContext()).toBeNull();
  });

  it("is null when the answer is a shape we cannot read", async () => {
    meAnswers({ organization: { id: "x" } });
    expect(await getServerContext()).toBeNull();
  });

  // Which organization a request acts in is auth's to say. An answer without
  // it is unreadable, not an invitation to pick one here.
  it("is null when the answer does not say which organization to act in", async () => {
    meAnswers({ person: PERSON, organizations: [OWNED, JOINED] });
    expect(await getServerContext()).toBeNull();
  });

  // ⚠ **Saying "none" is an answer; not saying is not.** Sign-ups closed, a
  // person in no organization is signed in to their account alone, and the
  // layout needs this context — their invitations and flags — to show them
  // it. Read as an outage, they would get a broken console instead.
  it("reads an answer that places the person in no organization", async () => {
    meAnswers(me({ organizations: [], activeOrganizationId: null, flags: { signup: false } }));
    const ctx = await getServerContext();
    expect(ctx).not.toBeNull();
    expect(ctx?.activeOrganizationId).toBeNull();
    expect(ctx?.organizations).toEqual([]);
    expect(ctx?.flags).toEqual({ signup: false });
    expect(ctx && activeOrganization(ctx)).toBeNull();
  });
});

describe("activeOrganization", () => {
  const ctx = (activeOrganizationId: string) =>
    ({
      person: PERSON,
      organizations: [OWNED, JOINED],
      deletedOrganizations: [],
      activeOrganizationId,
      memberships: [],
      incomingInvites: [],
      projectOffers: [],
      flags: {},
    }) as ServerContext;

  it("is the entry auth resolved, whatever the person's role in it", () => {
    expect(activeOrganization(ctx("org_1"))).toBe(OWNED);
    expect(activeOrganization(ctx("org_2"))).toBe(JOINED);
  });

  it("is null when auth names an organization its own list does not hold", () => {
    expect(activeOrganization(ctx("org_gone"))).toBeNull();
  });
});
