import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/data", () => ({
  getServerContext: vi.fn(),
  fetchProjectBySlug: vi.fn(),
  traceDownload: vi.fn(),
}));

import { NextRequest } from "next/server";

import { fetchProjectBySlug, getServerContext, traceDownload } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = { id: "project_7bQx2mNv9BcK4dLp", slug: "web" };

/** A download with the query given. */
function exported(query: string) {
  return GET(new NextRequest(`http://localhost:3000/acme/web/traces/export?format=csv&${query}`), {
    params: Promise.resolve({ organization: "acme", project: "web" }),
  });
}

describe("GET traces/export", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
    vi.mocked(getServerContext).mockResolvedValue({} as never);
    vi.mocked(fetchProjectBySlug).mockResolvedValue(PROJECT as never);
    vi.mocked(traceDownload).mockResolvedValue({
      kind: "ok",
      body: "trace_id\n",
      contentType: "text/csv",
    } as never);
  });

  it("refuses a filter value the list would let go rather than download every row", async () => {
    const session = "s".repeat(200);
    expect((await exported(`session=${session}`)).status).toBe(200);
    expect(vi.mocked(traceDownload).mock.calls[0]?.[1]).toEqual({ session: [session] });
    for (const query of [`session=${session}s`, "status=gone", "tag=no-colon"]) {
      expect((await exported(query)).status).toBe(400);
    }
    expect(traceDownload).toHaveBeenCalledTimes(1);
  });
});
