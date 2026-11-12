import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/data", () => ({
  getServerContext: vi.fn(),
  fetchProjectBySlug: vi.fn(),
}));
vi.mock("@/lib/server/entities/users", () => ({ userDownload: vi.fn() }));

import { NextRequest } from "next/server";

import { fetchProjectBySlug, getServerContext } from "@/lib/server/data";
import { userDownload } from "@/lib/server/entities/users";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = { id: "project_7bQx2mNv9BcK4dLp", slug: "web" };

/** A download with the query given. */
function exported(query: string) {
  return GET(new NextRequest(`http://localhost:3000/acme/web/users/export?${query}`), {
    params: Promise.resolve({ organization: "acme", project: "web" }),
  });
}

describe("GET users/export", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
    vi.mocked(getServerContext).mockResolvedValue({} as never);
    vi.mocked(fetchProjectBySlug).mockResolvedValue(PROJECT as never);
    vi.mocked(userDownload).mockResolvedValue({
      kind: "ok",
      body: "user_id\n",
      contentType: "text/csv",
    } as never);
  });

  it("writes the list its URL names as a file of the project's own name, kept out of caches", async () => {
    const res = await exported("format=json&agent=triage&sort=sessions&dir=asc&days=30&page=3");
    expect(res.status).toBe(200);
    expect(userDownload).toHaveBeenCalledWith(
      PROJECT.id,
      { agent: ["triage"], sort: "sessions", dir: "asc", days: 30, page: 3 },
      "json",
    );
    expect(res.headers.get("content-disposition")).toMatch(/^attachment; filename="web-users-\d{4}-\d\d-\d\d\.json"$/);
    expect(res.headers.get("cache-control")).toBe("no-store, private");
  });

  it("refuses a filter value the list would let go rather than download every row, and a sort it has none of", async () => {
    for (const query of ["status=gone", "tag=no-colon", `user=${"u".repeat(129)}`]) {
      expect((await exported(query)).status, query).toBe(400);
    }
    expect(userDownload).not.toHaveBeenCalled();
    // A sort of another list's is dropped, as the page drops it: the server's own stands.
    await exported("sort=started");
    expect(vi.mocked(userDownload).mock.calls[0]?.[1]).toEqual({});
  });
});
