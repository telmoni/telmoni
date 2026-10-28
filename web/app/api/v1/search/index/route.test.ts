import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/data", () => ({
  fetchMembers: vi.fn(),
  fetchProjectAnywhere: vi.fn(),
  fetchTokens: vi.fn(),
}));

import { NextRequest } from "next/server";

import { rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { fetchMembers, fetchProjectAnywhere, fetchTokens } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = "project_7bQx2mNv9BcK4dLp";

function get(project: string | null = PROJECT) {
  const query = project === null ? "" : `?project=${encodeURIComponent(project)}`;
  return GET(new NextRequest(`http://localhost:3000/api/v1/search/index${query}`));
}

describe("GET /api/v1/search/index", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ accessToken: "at_1" } as never);
    vi.mocked(rateLimitRetryAfter).mockResolvedValue(null);
    vi.mocked(fetchProjectAnywhere).mockResolvedValue({
      id: PROJECT,
      slug: "web",
      organizationId: "org_1",
      organizationSlug: "acme",
    } as never);
    vi.mocked(fetchTokens).mockResolvedValue({
      kind: "ok",
      tokens: [{ id: "tok_1", name: "Deploy" }],
    } as never);
    vi.mocked(fetchMembers).mockResolvedValue({
      kind: "ok",
      members: [{ memberId: "user_1", email: "ada@example.test", displayName: "Ada" }],
    } as never);
  });

  it("indexes the project's keys and members, each leading to its page, for this session alone", async () => {
    const res = await get();
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual({
      keys: {
        items: [{ id: "key:tok_1", kind: "key", label: "Deploy", hint: "API key", href: "/acme/web/api-keys" }],
        forbidden: false,
        unavailable: false,
      },
      members: {
        items: [
          { id: "member:user_1", kind: "member", label: "ada@example.test", hint: "Ada", href: "/acme/web/members" },
        ],
        forbidden: false,
        unavailable: false,
      },
    });
  });

  it("says which sections the reader may not see, and which could not be read", async () => {
    vi.mocked(fetchTokens).mockResolvedValue({ kind: "forbidden" } as never);
    vi.mocked(fetchMembers).mockResolvedValue({ kind: "unavailable" } as never);
    const body = await (await get()).json();
    expect(body.keys).toEqual({ items: [], forbidden: true, unavailable: false });
    expect(body.members).toEqual({ items: [], forbidden: false, unavailable: true });
  });

  it("refuses a session that has ended", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null as never);
    const res = await get();
    expect(res.status).toBe(401);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual({
      type: "about:blank",
      title: "Your session expired — sign in again.",
      status: 401,
    });
    expect(fetchTokens).not.toHaveBeenCalled();
  });

  it("refuses past the limit, reading nothing", async () => {
    vi.mocked(rateLimitRetryAfter).mockResolvedValue(30);
    const res = await get();
    expect(res.status).toBe(429);
    expect(fetchProjectAnywhere).not.toHaveBeenCalled();
  });

  // The path names no organization and the cookie follows the last tab, so a
  // project is placed in its own organization, never the cookie's.
  it("leads to the project's pages in its own organization", async () => {
    vi.mocked(fetchProjectAnywhere).mockResolvedValue({
      id: PROJECT,
      slug: "api",
      organizationId: "org_2",
      organizationSlug: "globex",
    } as never);
    const body = await (await get()).json();
    expect(body.keys.items[0].href).toBe("/globex/api/api-keys");
    expect(body.members.items[0].href).toBe("/globex/api/members");
  });

  it("refuses anything but a project id, and a project the session cannot place", async () => {
    for (const project of [null, "", "../members", "org_1"]) {
      expect((await get(project)).status).toBe(404);
    }
    expect(fetchProjectAnywhere).not.toHaveBeenCalled();

    vi.mocked(fetchProjectAnywhere).mockResolvedValue(null as never);
    const res = await get();
    expect(res.status).toBe(404);
    expect(await res.json()).toEqual({ type: "about:blank", title: "Unknown project.", status: 404 });
    expect(fetchTokens).not.toHaveBeenCalled();
  });
});
