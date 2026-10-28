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

/** A download of the runs named, as the page's chosen rows ask for it. */
function download(...runs: string[]) {
  return exported(runs.map((run) => `run=${encodeURIComponent(run)}`).join("&"));
}

describe("GET traces/export", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
    vi.mocked(getServerContext).mockResolvedValue({} as never);
    vi.mocked(fetchProjectBySlug).mockResolvedValue(PROJECT as never);
    vi.mocked(traceDownload).mockResolvedValue({
      kind: "ok",
      body: "run_id\n",
      contentType: "text/csv",
    } as never);
  });

  it("names a chosen run of any UUID version, as ingest takes it", async () => {
    const run = "3d6f8a1c-5b2e-0f7a-0c0d-8e1f2a3b4c5d";
    expect((await download(run)).status).toBe(200);
    expect(vi.mocked(traceDownload).mock.calls[0]?.[3]).toEqual([run]);
  });

  it("refuses a chosen run that is no UUID rather than download every row", async () => {
    expect((await download("3d6f8a1c-5b2e-4f7a-9c0d-8e1f2a3b4c5d", "../admin")).status).toBe(400);
    expect(traceDownload).not.toHaveBeenCalled();
  });

  it("names every run of a page's hundred chosen, and refuses a hundred and one", async () => {
    const runs = Array.from(
      { length: 101 },
      (_, n) => `3d6f8a1c-5b2e-4f7a-9c0d-${n.toString(16).padStart(12, "0")}`,
    );
    expect((await download(...runs.slice(0, 100))).status).toBe(200);
    expect(vi.mocked(traceDownload).mock.calls[0]?.[3]).toEqual(runs.slice(0, 100));
    expect((await download(...runs)).status).toBe(400);
    expect(traceDownload).toHaveBeenCalledTimes(1);
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
