import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({
    message: "Conflict: this project has a monitor by that slug already",
  })),
}));
vi.mock("@/lib/env", () => ({
  env: { SERVER_URL: "http://telemetry.test" },
}));
vi.mock("@/lib/server/identity", () => ({
  activeProjectForMutation: vi.fn(async (projectId: string) => ({
    organizationId: "org_1",
    projectId,
    error: null,
  })),
}));
vi.mock("@/lib/server/entities/identity-context", () => ({
  sessionHeaders: () => ({ authorization: "Bearer at_1" }),
}));
const mockRefresh = vi.fn();
vi.mock("next/cache", () => ({
  refresh: (...a: unknown[]) => mockRefresh(...a),
}));

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimit } from "@/lib/api/rate-limit";
import { getServerSession } from "@/lib/server/session";

import {
  createMonitorAction,
  deleteMonitorAction,
  mintPingKeyAction,
  pauseMonitorAction,
  previewScheduleAction,
  retirePingKeyAction,
  updateMonitorAction,
} from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);
const PROJECT = "project_1";
const BODY = { name: "Nightly report", slug: "nightly-report", bounds: { max_steps: 40 } };
const MONITOR = {
  id: "0d2b7a6e-6c0f-4a1b-9f3e-1c2d3e4f5a6b",
  slug: "nightly-report",
  name: "Nightly report",
  agent: "nightly-report",
  schedule: null,
  interval_s: null,
  timezone: "UTC",
  grace_s: null,
  expected_duration_s: null,
  bounds: { stall_after_s: null, max_steps: 40, max_cost_usd: null },
  state: "healthy",
  state_changed_at: "2026-10-22T00:00:00Z",
  next_due_at: null,
  last_run: null,
  last_success_at: null,
  created_at: "2026-10-22T00:00:00Z",
  updated_at: "2026-10-22T00:00:00Z",
};

function answer(status: number, body: unknown) {
  fetchMock.mockResolvedValue(
    new Response(body === null ? null : JSON.stringify(body), { status }),
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1" } as never);
  answer(201, MONITOR);
});

describe("the monitor actions", () => {
  it("make a monitor on telemetry's lane as the person, and draw the pages again", async () => {
    expect(await createMonitorAction(PROJECT, BODY)).toEqual({ status: "ok", monitor: MONITOR });
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe("http://telemetry.test/internal/v1/telemetry/monitors");
    expect(init?.method).toBe("POST");
    expect(init?.headers).toMatchObject({ authorization: "Bearer at_1" });
    expect(JSON.parse(String(init?.body))).toEqual(BODY);
    expect(mockRefresh).toHaveBeenCalledTimes(1);
  });

  it("forward nothing but the lane's shape, nor a slug of no monitor's shape", async () => {
    for (const body of [
      { ...BODY, owner: "user_2" },
      { ...BODY, bounds: { max_tokens: 1 } },
      { name: 1, bounds: {} },
      "nightly",
    ]) {
      expect(await createMonitorAction(PROJECT, body)).toEqual({
        status: "error",
        error: "That monitor could not be read.",
      });
    }
    expect(await updateMonitorAction(PROJECT, "../ping-keys", BODY)).toMatchObject({
      status: "error",
    });
    expect(await pauseMonitorAction(PROJECT, "Nightly")).toMatchObject({ status: "error" });
    expect(await deleteMonitorAction(PROJECT, "a/b")).toEqual({
      error: "That monitor could not be read.",
    });
    expect(await retirePingKeyAction(PROJECT, "key")).toEqual({ error: "Invalid key id." });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("change, pause and delete the monitor its slug names", async () => {
    answer(200, MONITOR);
    await updateMonitorAction(PROJECT, "nightly-report", BODY);
    await pauseMonitorAction(PROJECT, "nightly-report");
    answer(204, null);
    expect(await deleteMonitorAction(PROJECT, "nightly-report")).toEqual({});
    expect(fetchMock.mock.calls.map(([url, init]) => [init?.method, url])).toEqual([
      ["PUT", "http://telemetry.test/internal/v1/telemetry/monitors/nightly-report"],
      ["POST", "http://telemetry.test/internal/v1/telemetry/monitors/nightly-report/pause"],
      ["DELETE", "http://telemetry.test/internal/v1/telemetry/monitors/nightly-report"],
    ]);
  });

  it("say what the lane refused, in its words", async () => {
    answer(409, { title: "Conflict" });
    expect(await createMonitorAction(PROJECT, BODY)).toEqual({
      status: "error",
      error: "Conflict: this project has a monitor by that slug already",
    });
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("refuse while signed out or past the minute's limit, asking nothing", async () => {
    vi.mocked(getServerSession).mockResolvedValueOnce(null);
    expect(await createMonitorAction(PROJECT, BODY)).toMatchObject({ status: "error" });
    vi.mocked(rateLimit).mockResolvedValueOnce({ retryAfter: 30 } as never);
    expect(await createMonitorAction(PROJECT, BODY)).toMatchObject({ status: "error" });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("ask what a schedule means without saving, and draw nothing again", async () => {
    answer(200, { next: ["2026-10-23T01:00:00Z"] });
    expect(
      await previewScheduleAction(PROJECT, { schedule: "0 2 * * *", timezone: "Europe/London" }),
    ).toEqual({ status: "ok", next: ["2026-10-23T01:00:00Z"] });
    expect(fetchMock.mock.calls[0]![0]).toBe(
      "http://telemetry.test/internal/v1/telemetry/monitors/schedule",
    );
    expect(mockRefresh).not.toHaveBeenCalled();
    expect(await previewScheduleAction(PROJECT, { cron: "0 2 * * *" })).toMatchObject({
      status: "error",
    });
  });

  it("mint a ping key, shown this once", async () => {
    answer(201, {
      key: "telmoniping_Ab3dE6gH9jKmNpQrStUvWxYz01234567",
      id: "1b2c0000-0000-4000-8000-000000000001",
      prefix: "telmoniping_Ab3dE6gH",
      created_at: "2026-10-22T00:00:00Z",
      last_ping_at: null,
      retired_at: null,
    });
    expect(await mintPingKeyAction(PROJECT)).toEqual({
      status: "ok",
      key: "telmoniping_Ab3dE6gH9jKmNpQrStUvWxYz01234567",
      prefix: "telmoniping_Ab3dE6gH",
    });
    expect(fetchMock.mock.calls[0]![0]).toBe("http://telemetry.test/internal/v1/telemetry/ping-keys");
    expect(mockRefresh).toHaveBeenCalledTimes(1);
  });
});
