import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/data", () => ({
  getServerContext: vi.fn(),
  fetchProjectBySlug: vi.fn(),
  userDownload: vi.fn(),
}));

import { NextRequest } from "next/server";

import { sessionKey } from "@/lib/api/rate-limit";
import { fetchProjectBySlug, getServerContext, userDownload } from "@/lib/server/data";
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
      body: new TextEncoder().encode("user_id\n").buffer,
      contentType: "text/csv; charset=utf-8",
    } as never);
  });

  it("writes the users the filters admit, as CSV unless JSON is asked for, under the project's and the list's name", async () => {
    const res = await exported("model=claude-sonnet-5-5&sort=errors&dir=asc&format=xlsx");
    expect(res.status).toBe(200);
    expect(vi.mocked(userDownload).mock.calls[0]).toEqual([
      PROJECT.id,
      { model: ["claude-sonnet-5-5"], sort: "errors", dir: "asc" },
      "csv",
    ]);
    expect(res.headers.get("content-disposition")).toMatch(
      /^attachment; filename="web-users-\d{4}-\d{2}-\d{2}\.csv"$/,
    );
    expect(res.headers.get("content-type")).toBe("text/csv; charset=utf-8");
    expect(sessionKey).toHaveBeenCalledWith({ userId: "user_1" }, "users:download");
  });

  it("refuses a filter value the list would let go rather than download every user", async () => {
    expect((await exported(`user=${"u".repeat(129)}`)).status).toBe(400);
    expect(userDownload).not.toHaveBeenCalled();
  });
});
