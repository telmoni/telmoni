import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/data", () => ({ fetchTrace: vi.fn(), fetchTraceIncidents: vi.fn() }));
vi.mock("@/lib/server/entities/scores", () => ({ fetchScores: vi.fn() }));

import { NextRequest } from "next/server";

import { rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { fetchTraceIncidents, fetchTrace } from "@/lib/server/data";
import { fetchScores } from "@/lib/server/entities/scores";
import { getServerSession } from "@/lib/server/session";
import { PeekTraceSchema } from "@/lib/types/traces";

import { GET } from "./route";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const TRACE_ID = "5d4f5c68000040008000000000000001";
const TRACE = { trace_id: TRACE_ID, spans: [], truncated: false, content: [] };

/** The peek's own read. */
function read(trace = TRACE_ID, project: string | null = PROJECT) {
  const query = project === null ? "" : `?project=${encodeURIComponent(project)}`;
  return GET(new NextRequest(`http://localhost:3000/api/v1/traces/${trace}${query}`), {
    params: Promise.resolve({ trace }),
  });
}

describe("GET /api/v1/traces/[trace]", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
    vi.mocked(fetchTrace).mockResolvedValue({ kind: "ok", trace: TRACE } as never);
    vi.mocked(fetchTraceIncidents).mockResolvedValue([]);
    vi.mocked(fetchScores).mockResolvedValue({ kind: "ok", scores: [] });
  });

  it("answers the project's trace, never to be kept by a cache", async () => {
    const res = await read();
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual({ ...TRACE, incidents: [], scores: [] });
    expect(fetchTrace).toHaveBeenCalledWith(PROJECT, TRACE_ID, true);
  });

  // The peek's line as the trace's page draws it: the incidents named as the
  // line names them, and no scores where they could not be read.
  it("answers the trace's incidents by name and its scores with its spans", async () => {
    vi.mocked(fetchTraceIncidents).mockResolvedValue([
      { id: "inc_1", rule_name: "Looping traces", agent: null, status: "open" },
      { id: "inc_2", rule_name: null, agent: "support-bot", status: "open" },
    ] as never);
    vi.mocked(fetchScores).mockResolvedValue({ kind: "unavailable" });
    const body = await (await read()).json();
    expect(body.incidents).toEqual([
      { id: "inc_1", rule_name: "Looping traces", agent: null },
      { id: "inc_2", rule_name: null, agent: "support-bot" },
    ]);
    // An alert's incident and a monitor's both read in the browser.
    expect(PeekTraceSchema.shape.incidents.safeParse(body.incidents).success).toBe(true);
    expect(body.scores).toBeNull();
  });

  it("refuses a request with no sign-in before reading anything", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null as never);
    expect((await read()).status).toBe(401);
    expect(fetchTrace).not.toHaveBeenCalled();
  });

  it("holds a reader to their pace", async () => {
    vi.mocked(rateLimitRetryAfter).mockResolvedValueOnce(30);
    expect((await read()).status).toBe(429);
    expect(fetchTrace).not.toHaveBeenCalled();
  });

  it("takes a trace id with its leading zeros, as ingest does", async () => {
    const trace = "00000000000000000000000000000001";
    expect((await read(trace)).status).toBe(200);
    expect(fetchTrace).toHaveBeenCalledWith(PROJECT, trace, true);
  });

  it("sends on nothing but a project's id and a trace's 32 hex digits", async () => {
    expect((await read(TRACE_ID, null)).status).toBe(404);
    expect((await read(TRACE_ID, "../admin")).status).toBe(404);
    expect((await read("..%2F..%2Fadmin")).status).toBe(404);
    expect((await read("5d4f5c68-0000-4000-8000-000000000001")).status).toBe(404);
    expect((await read("5d4f5c6800004000800000000000001")).status).toBe(404);
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
