import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/data", () => ({ fetchTrace: vi.fn() }));

import { NextRequest, NextResponse } from "next/server";

import { rateLimit } from "@/lib/api/rate-limit";
import { fetchTrace } from "@/lib/server/data";
import { getServerSession } from "@/lib/server/session";

import { GET } from "./route";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const RUN = "5d4f5c68-0000-4000-8000-000000000001";
const TRACE = { run_id: RUN, spans: [], truncated: false };

/** The peek's own read. */
function read(run = RUN, project: string | null = PROJECT) {
  const query = project === null ? "" : `?project=${encodeURIComponent(project)}`;
  return GET(new NextRequest(`http://localhost:3000/api/traces/${run}${query}`), {
    params: Promise.resolve({ run }),
  });
}

describe("GET /api/traces/[run]", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
    vi.mocked(fetchTrace).mockResolvedValue({ kind: "ok", trace: TRACE } as never);
  });

  it("answers the project's trace, never to be kept by a cache", async () => {
    const res = await read();
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual(TRACE);
    expect(fetchTrace).toHaveBeenCalledWith(PROJECT, RUN);
  });

  it("refuses a request with no sign-in before reading anything", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null as never);
    expect((await read()).status).toBe(401);
    expect(fetchTrace).not.toHaveBeenCalled();
  });

  it("holds a reader to their pace", async () => {
    vi.mocked(rateLimit).mockResolvedValueOnce(NextResponse.json({}, { status: 429 }));
    expect((await read()).status).toBe(429);
    expect(fetchTrace).not.toHaveBeenCalled();
  });

  it("takes a UUID of any version, as ingest does", async () => {
    const run = "11111111-1111-1111-1111-111111111111";
    expect((await read(run)).status).toBe(200);
    expect(fetchTrace).toHaveBeenCalledWith(PROJECT, run);
  });

  it("sends on nothing but a project's id and a run's UUID", async () => {
    expect((await read(RUN, null)).status).toBe(404);
    expect((await read(RUN, "../admin")).status).toBe(404);
    expect((await read("..%2F..%2Fadmin")).status).toBe(404);
    expect(fetchTrace).not.toHaveBeenCalled();
  });

  it("answers each way the read can end", async () => {
    vi.mocked(fetchTrace).mockResolvedValueOnce({ kind: "missing" } as never);
    expect((await read()).status).toBe(404);
    vi.mocked(fetchTrace).mockResolvedValueOnce({ kind: "forbidden" } as never);
    expect((await read()).status).toBe(403);
    vi.mocked(fetchTrace).mockResolvedValueOnce({ kind: "unavailable" } as never);
    expect((await read()).status).toBe(503);
  });
});
