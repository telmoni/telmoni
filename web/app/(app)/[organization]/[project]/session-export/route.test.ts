import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/data", () => ({
  getServerContext: vi.fn(),
  fetchProjectBySlug: vi.fn(),
  sessionDownload: vi.fn(),
}));

import { NextRequest } from "next/server";

import { sessionKey } from "@/lib/api/rate-limit";
import { fetchProjectBySlug, getServerContext, sessionDownload } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = { id: "project_7bQx2mNv9BcK4dLp", slug: "web" };

/** A download with the query given, from `site`. */
function exported(query: string, site = "same-origin") {
  return GET(
    new NextRequest(`http://localhost:3000/acme/web/session-export?${query}`, {
      headers: { "sec-fetch-site": site },
    }),
    { params: Promise.resolve({ organization: "acme", project: "web" }) },
  );
}

describe("GET session-export", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
    vi.mocked(getServerContext).mockResolvedValue({} as never);
    vi.mocked(fetchProjectBySlug).mockResolvedValue(PROJECT as never);
    vi.mocked(sessionDownload).mockResolvedValue({
      kind: "ok",
      body: new TextEncoder().encode("[]").buffer,
      contentType: "application/json",
    } as never);
  });

  it("writes the sessions the filters admit, sorted as the list is, under the project's and the list's name", async () => {
    const res = await exported("format=json&status=error&sort=tokens&page=3&limit=25");
    expect(res.status).toBe(200);
    expect(vi.mocked(sessionDownload).mock.calls[0]).toEqual([
      PROJECT.id,
      { status: ["error"], sort: "tokens", page: 3, limit: 25 },
      "json",
    ]);
    expect(res.headers.get("content-disposition")).toMatch(
      /^attachment; filename="web-sessions-\d{4}-\d{2}-\d{2}\.json"$/,
    );
    expect(res.headers.get("cache-control")).toBe("no-store, private");
    expect(sessionKey).toHaveBeenCalledWith({ userId: "user_1" }, "sessions:download");
  });

  it("refuses a filter value the list would let go rather than download every session", async () => {
    for (const query of [`session=${"s".repeat(201)}`, "status=gone", "tag=no-colon"]) {
      expect((await exported(query)).status).toBe(400);
    }
    expect(sessionDownload).not.toHaveBeenCalled();
  });

  it("answers only the console's own pages, and only a person signed in", async () => {
    expect((await exported("format=csv", "cross-site")).status).toBe(403);
    vi.mocked(getServerSession).mockResolvedValue(null as never);
    expect((await exported("format=csv")).status).toBe(401);
    expect(sessionDownload).not.toHaveBeenCalled();
  });

  it("says what the server said: a query refused, a role refused, an outage", async () => {
    for (const [kind, status] of [
      ["refused", 400],
      ["forbidden", 403],
      ["unavailable", 503],
    ] as const) {
      vi.mocked(sessionDownload).mockResolvedValueOnce({ kind } as never);
      expect((await exported("format=csv")).status).toBe(status);
    }
  });
});
