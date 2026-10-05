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

import {
  changeOrganizationUrlAction,
  nameOrganizationAction,
  renameOrganizationAction,
} from "./settings-actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

// The organization the settings page rendered, and handed to the form.
const ORGANIZATION = "org_acme";
const SWITCHED =
  "This page is out of date. Reload it to act on the organization you're viewing.";

// Auth's answer to the organization's settings: the name as stored, and the
// slug it goes by now.
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

describe("nameOrganizationAction", () => {
  const UNNAMED = { organizationId: ORGANIZATION, slug: "org-k3x9qz1a2b", name: null };

  // ⚠ Posted from `/console`, which stands on no organization's path: the
  // request resolves the cookie's organization — another one, when the owner
  // opened their unnamed organization from inside it — so the action acts on
  // the organization it was handed, never on the one the request resolves.
  it("names the organization it was handed, whatever the request resolves to", async () => {
    standingIn("org_elsewhere");
    vi.mocked(getServerContext).mockResolvedValue({
      activeOrganizationId: "org_elsewhere",
      organizations: [
        { organizationId: "org_elsewhere", slug: "elsewhere", name: "Elsewhere", role: "owner" },
        { ...UNNAMED, role: "owner" },
      ],
    } as never);
    fetchMock.mockResolvedValue(renamed("acme-robotics"));
    expect(await nameOrganizationAction(ORGANIZATION, "  Acme Robotics  ")).toEqual({
      error: null,
      movedTo: "acme-robotics",
      slug: "acme-robotics",
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/organization",
      expect.objectContaining({
        method: "PATCH",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": ORGANIZATION,
        }),
        body: JSON.stringify({ name: "Acme Robotics" }),
      }),
    );
    expect(identityContext).not.toHaveBeenCalled();
    expect(mockPublishEvent).toHaveBeenCalledWith(`bfev:organization:${ORGANIZATION}`, {
      type: "slug:moved",
      data: { organizationId: ORGANIZATION, from: "org-k3x9qz1a2b", to: "acme-robotics" },
    });
  });

  it("refuses an organization the person does not own, before asking auth", async () => {
    vi.mocked(getServerContext).mockResolvedValue({
      activeOrganizationId: ORGANIZATION,
      organizations: [{ ...UNNAMED, role: "admin" }],
    } as never);
    const res = await nameOrganizationAction(ORGANIZATION, "Acme");
    expect(res.error).toMatch(/owner/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // A name with no Latin letter or digit gives no slug: the placeholder stays, so
  // nobody is told of a move — but the form still needs the slug to land on,
  // since `/console` would open the cookie's organization instead.
  it("answers the slug without a move when the name gave the organization no URL", async () => {
    vi.mocked(getServerContext).mockResolvedValue({
      activeOrganizationId: ORGANIZATION,
      organizations: [{ ...UNNAMED, role: "owner" }],
    } as never);
    fetchMock.mockResolvedValue(renamed("org-k3x9qz1a2b"));
    expect(await nameOrganizationAction(ORGANIZATION, "東京")).toEqual({
      error: null,
      slug: "org-k3x9qz1a2b",
    });
    expect(mockPublishEvent).not.toHaveBeenCalled();
  });

  // The first name is this action's; a stale `/console` tab posting a second
  // one would rename the organization without anyone seeing it as a rename.
  it("refuses an organization that already has a name, before asking auth", async () => {
    vi.mocked(getServerContext).mockResolvedValue({
      activeOrganizationId: ORGANIZATION,
      organizations: [{ ...UNNAMED, name: "Acme", slug: "acme", role: "owner" }],
    } as never);
    const res = await nameOrganizationAction(ORGANIZATION, "Acme Robotics");
    expect(res.error).toMatch(/already has a name/);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("renameOrganizationAction", () => {
  it("renames the organization the page rendered, under the session's bearer", async () => {
    fetchMock.mockResolvedValue(renamed("acme-robotics"));
    expect(await renameOrganizationAction(ORGANIZATION, "  Acme Robotics  ")).toEqual({
      error: null,
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/organization",
      expect.objectContaining({
        method: "PATCH",
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

  // ⚠ The first name takes the organization off its placeholder slug, out
  // from under the path this was posted from. The page follows it; rendering
  // the old path again first, as a revalidation would, answers "not found".
  // After that a name moves nothing, which the test above pins.
  it("says where the first name moved the organization, and revalidates nothing", async () => {
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
  it("tells the organization's channel where the first name moved it", async () => {
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

  // ⚠ The organization this page rendered has changed its URL since, so its
  // path names nobody and `/me` answers another one at the click. Sent on, the name
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
  // tab changed its URL, and this one missed the move: its path names an
  // organization auth did not answer with, and a reload would answer "not
  // found", so "try again" would be the wrong advice.
  it("tells a tab that missed the move that its address is gone", async () => {
    vi.mocked(identityContext).mockResolvedValue(null);
    vi.mocked(getServerContext).mockResolvedValue({
      organizationNotFound: true,
    } as never);
    const res = await renameOrganizationAction(ORGANIZATION, "Acme Robotics");
    expect(res.error).toMatch(/URL was changed, or you're no longer in it/);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

// The URL is its own setting, as on Vercel's team page: a slug an owner or
// admin chooses, which auth holds to being free and none of the console's own
// words.
describe("changeOrganizationUrlAction", () => {
  it("changes the URL of the organization the page rendered, and says where it went", async () => {
    fetchMock.mockResolvedValue(renamed("acme"));
    expect(await changeOrganizationUrlAction(ORGANIZATION, " acme ")).toEqual({
      error: null,
      movedTo: "acme",
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://auth.test/internal/organization",
      expect.objectContaining({
        method: "PATCH",
        headers: expect.objectContaining({
          authorization: "Bearer at_1",
          "x-organization-id": ORGANIZATION,
        }),
        body: JSON.stringify({ slug: "acme" }),
      }),
    );
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  // Everybody with one of the organization's pages open is on a path that now
  // names nothing: the organization's channel tells them where it went.
  it("tells the organization's channel where the URL moved", async () => {
    fetchMock.mockResolvedValue(renamed("acme"));
    await changeOrganizationUrlAction(ORGANIZATION, "acme");
    expect(mockPublishEvent).toHaveBeenCalledTimes(1);
    expect(mockPublishEvent).toHaveBeenCalledWith(`bfev:organization:${ORGANIZATION}`, {
      type: "slug:moved",
      data: { organizationId: ORGANIZATION, from: "acme-robotics", to: "acme" },
    });
  });

  it("revalidates in place when auth kept the URL it had", async () => {
    fetchMock.mockResolvedValue(renamed("acme-robotics"));
    expect(await changeOrganizationUrlAction(ORGANIZATION, "acme-robotics")).toEqual({
      error: null,
    });
    expect(mockRevalidate).toHaveBeenCalledWith("/", "layout");
    expect(mockPublishEvent).not.toHaveBeenCalled();
  });

  // What a URL is, said here so the form can say it; whether it is free is
  // auth's to say.
  it.each(["Acme", "acme_robotics", "-acme", "acme--robotics", "a".repeat(49)])(
    "refuses %s, which is not a slug, without asking auth",
    async (slug) => {
      const res = await changeOrganizationUrlAction(ORGANIZATION, slug);
      expect(res.error).toMatch(/lowercase letters and digits/);
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  it.each(["account", "api", "settings", "console"])(
    "refuses %s, a word the console's own pages use, without asking auth",
    async (slug) => {
      const res = await changeOrganizationUrlAction(ORGANIZATION, slug);
      expect(res.error).toMatch(/console's own pages/);
      expect(fetchMock).not.toHaveBeenCalled();
    },
  );

  it("passes on auth's refusal of a URL another organization holds, telling nobody", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 409 }));
    expect(await changeOrganizationUrlAction(ORGANIZATION, "globex")).toEqual({
      error: "problem",
    });
    expect(mockPublishEvent).not.toHaveBeenCalled();
    expect(mockRevalidate).not.toHaveBeenCalled();
  });

  it("refuses without asking auth when the request resolves another organization", async () => {
    standingIn("org_elsewhere");
    expect(await changeOrganizationUrlAction(ORGANIZATION, "acme")).toEqual({
      error: SWITCHED,
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
