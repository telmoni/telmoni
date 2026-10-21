import { beforeEach, describe, expect, it, vi } from "vitest";

const { fetchWithTimeout, identityContext } = vi.hoisted(() => ({
  fetchWithTimeout: vi.fn(),
  identityContext: vi.fn(),
}));

vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://server" } }));
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("./identity-context", () => ({
  identityContext,
  projectHeaders: (_ctx: unknown, projectId: string) => ({
    authorization: "Bearer at_1",
    "x-project-id": projectId,
  }),
}));
import { fetchMonitors } from "./monitors";

const PROJECT = "project_1";

const MONITOR = {
  id: "0d2b7a6e-6c0f-4a1b-9f3e-1c2d3e4f5a6b",
  slug: "nightly-report",
  name: "Nightly report",
  agent: "nightly-report",
  schedule: "0 2 * * *",
  interval_s: null,
  timezone: "Europe/London",
  grace_s: 600,
  expected_duration_s: 300,
  bounds: { stall_after_s: 120, max_steps: 40, max_cost_usd: null },
  state: "failed",
  state_changed_at: "2026-10-21T02:10:00Z",
  next_due_at: "2026-10-22T01:00:00Z",
  last_run: {
    run_id: "5d4f5c68-0000-4000-8000-000000000001",
    started_at: "2026-10-21T02:00:00Z",
    ended_at: "2026-10-21T02:04:02Z",
    status: "error",
    steps: 12,
    cost_usd: 0.08,
  },
  created_at: "2026-10-01T00:00:00Z",
  updated_at: "2026-10-21T02:10:00Z",
};

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
  answer(200, { monitors: [MONITOR] });
});

describe("fetchMonitors", () => {
  it("reads the project's monitors from telemetry's lane, as the lane orders them", async () => {
    expect(await fetchMonitors(PROJECT)).toEqual({ kind: "ok", monitors: [MONITOR] });
    const [url, init] = fetchWithTimeout.mock.calls[0];
    expect(url).toBe("http://server/internal/v1/telemetry/monitors");
    expect(init.headers).toMatchObject({ "x-project-id": PROJECT });
    expect(init.cache).toBe("no-store");
  });

  it("reads a monitor no run of which has ended", async () => {
    answer(200, { monitors: [{ ...MONITOR, last_run: null }] });
    expect(await fetchMonitors(PROJECT)).toEqual({
      kind: "ok",
      monitors: [{ ...MONITOR, last_run: null }],
    });
  });

  it("is forbidden on a 403, and unavailable on anything else it cannot read", async () => {
    answer(403, { title: "Forbidden" });
    expect(await fetchMonitors(PROJECT)).toEqual({ kind: "forbidden" });
    answer(503, { title: "Unavailable" });
    expect(await fetchMonitors(PROJECT)).toEqual({ kind: "unavailable" });
    answer(200, { monitors: [{ ...MONITOR, state: "sleeping" }] });
    expect(await fetchMonitors(PROJECT)).toEqual({ kind: "unavailable" });
    answer(200, [MONITOR]);
    expect(await fetchMonitors(PROJECT)).toEqual({ kind: "unavailable" });
    fetchWithTimeout.mockRejectedValue(new Error("timed out"));
    expect(await fetchMonitors(PROJECT)).toEqual({ kind: "unavailable" });
  });

  it("asks nothing without a project or a signed-in person", async () => {
    expect(await fetchMonitors("")).toEqual({ kind: "unavailable" });
    identityContext.mockResolvedValue(null);
    expect(await fetchMonitors(PROJECT)).toEqual({ kind: "unavailable" });
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });
});
