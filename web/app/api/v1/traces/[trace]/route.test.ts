import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/data", () => ({ fetchTrace: vi.fn(), fetchRunIncidents: vi.fn() }));
vi.mock("@/lib/server/entities/scores", () => ({ fetchScores: vi.fn() }));

import { NextRequest } from "next/server";

import { rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { fetchRunIncidents, fetchTrace } from "@/lib/server/data";
import { fetchScores } from "@/lib/server/entities/scores";
import { getServerSession } from "@/lib/server/session";
import { PeekRunSchema } from "@/lib/types/traces";

import { GET } from "./route";

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const RUN = "5d4f5c68-0000-4000-8000-000000000001";
const TRACE = { run_id: RUN, spans: [], truncated: false, content: [] };

/** The peek's own read. */
function read(run = RUN, project: string | null = PROJECT) {
  const query = project === null ? "" : `?project=${encodeURIComponent(project)}`;
  return GET(new NextRequest(`http://localhost:3000/api/v1/traces/${run}${query}`), {
    params: Promise.resolve({ run }),
  });
}

describe("GET /api/v1/traces/[run]", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
    vi.mocked(fetchTrace).mockResolvedValue({ kind: "ok", trace: TRACE } as never);
    vi.mocked(fetchRunIncidents).mockResolvedValue([]);
    vi.mocked(fetchScores).mockResolvedValue({ kind: "ok", scores: [] });
  });

  it("answers the project's trace, never to be kept by a cache", async () => {
    const res = await read();
    expect(res.status).toBe(200);
    expect(res.headers.get("cache-control")).toBe("private, no-store");
    expect(await res.json()).toEqual({ ...TRACE, incidents: [], scores: [] });
    expect(fetchTrace).toHaveBeenCalledWith(PROJECT, RUN, true);
  });

  // The peek's line as the run's page draws it: the incidents named as the
  // line names them, and no scores where they could not be read.
  it("answers the run's incidents by name and its scores with its trace", async () => {
    vi.mocked(fetchRunIncidents).mockResolvedValue([
      { id: "inc_1", rule_name: "Looping runs", agent: null, status: "open" },
      { id: "inc_2", rule_name: null, agent: "support-bot", status: "open" },
    ] as never);
    vi.mocked(fetchScores).mockResolvedValue({ kind: "unavailable" });
    const body = await (await read()).json();
    expect(body.incidents).toEqual([
      { id: "inc_1", rule_name: "Looping runs", agent: null },
      { id: "inc_2", rule_name: null, agent: "support-bot" },
    ]);
    // An alert's incident and a monitor's both read in the browser.
    expect(PeekRunSchema.shape.incidents.safeParse(body.incidents).success).toBe(true);
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

  it("takes a UUID of any version, as ingest does", async () => {
    const run = "11111111-1111-1111-1111-111111111111";
    expect((await read(run)).status).toBe(200);
    expect(fetchTrace).toHaveBeenCalledWith(PROJECT, run, true);
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
