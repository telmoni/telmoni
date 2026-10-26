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
import { fetchAlerts, fetchIncident, fetchRunIncidents } from "./alerts";

const PROJECT = "project_1";
const INCIDENT = "5d4f5c68-0000-4000-8000-000000000001";
const RULE = "7a1c0e52-0000-4000-8000-000000000002";
const RUN = "9b2d1f63-0000-4000-8000-000000000003";
const CONNECTION = "3c4e2a74-0000-4000-8000-000000000004";

const INCIDENT_ROW = {
  id: INCIDENT,
  rule_id: RULE,
  rule_name: "Slow calls",
  status: "open",
  severity: "alert",
  value: 6200,
  opened_at: "2026-10-26T03:00:00Z",
  acknowledged_at: null,
  acknowledged_by: null,
  resolved_at: null,
  resolved_by: null,
  reopened: 0,
  told_to: [CONNECTION],
  runs: [RUN],
};
const ALERT = {
  id: RULE,
  name: "Slow calls",
  kind: "model_call",
  measure: "duration",
  aggregation: "p95",
  filters: { model: ["claude-sonnet-5-5"] },
  window: "hour",
  operator: "gt",
  alert_threshold: 5000,
  warning_threshold: 3000,
  missing_data: "zero",
  renotify_minutes: null,
  paused: null,
  paused_at: null,
  value: 6200,
  evaluated_at: "2026-10-26T03:00:00Z",
  severity: "alert",
  severity_since: "2026-10-26T03:00:00Z",
  connectors: [{ connection_id: CONNECTION, level: "warning", hours: null, muted: false }],
  created_at: "2026-10-25T03:00:00Z",
  updated_at: "2026-10-25T03:00:00Z",
};
const MUTE = {
  id: "1e2f3a4b-0000-4000-8000-000000000005",
  rule_id: null,
  starts_at: "2026-10-26T02:00:00Z",
  ends_at: "2026-10-26T04:00:00Z",
  created_by: "user_1",
  created_at: "2026-10-26T02:00:00Z",
  ended_by: null,
};
const PRECISION = {
  days: 7,
  useful: 3,
  noise: 1,
  ratio: 0.75,
  rules: [{ rule_id: RULE, rule_name: "Slow calls", useful: 3, noise: 1, ratio: 0.75 }],
};
const VOTES = { ...INCIDENT_ROW, useful: 2, noise: 1, yours: "noise" };

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

/** Each lane answered as `answers` says, by the path it was asked. */
function lanes(answers: Record<string, { status: number; body: unknown }>) {
  fetchWithTimeout.mockImplementation(async (url: string) => {
    const path = new URL(url).pathname.replace("/internal/v1/telemetry", "");
    const { status, body } = answers[path] ?? { status: 404, body: {} };
    return { ok: status < 400, status, json: async () => body };
  });
}

const PAGE = {
  "/rules": { status: 200, body: { rules: [ALERT] } },
  "/incidents": { status: 200, body: { incidents: [INCIDENT_ROW] } },
  "/mutes": { status: 200, body: { mutes: [MUTE] } },
  "/precision": { status: 200, body: PRECISION },
};

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
});

describe("fetchAlerts", () => {
  it("reads the project's alerts, incidents, windows and precision together", async () => {
    lanes(PAGE);
    const access = await fetchAlerts(PROJECT);
    expect(access).toMatchObject({
      kind: "ok",
      alerts: [{ name: "Slow calls", connectors: [{ connection_id: CONNECTION }] }],
      incidents: [{ id: INCIDENT, told_to: [CONNECTION], runs: [RUN] }],
      mutes: [{ rule_id: null }],
      precision: { ratio: 0.75 },
    });
    const asked = fetchWithTimeout.mock.calls.map(([url]) => url).sort();
    expect(asked).toEqual([
      "http://server/internal/v1/telemetry/incidents",
      "http://server/internal/v1/telemetry/mutes",
      "http://server/internal/v1/telemetry/precision",
      "http://server/internal/v1/telemetry/rules",
    ]);
    for (const [, init] of fetchWithTimeout.mock.calls) {
      expect(init.headers).toMatchObject({ "x-project-id": PROJECT });
    }
  });

  it("reads a 403 on any lane as forbidden, and any other failure as an outage", async () => {
    lanes({ ...PAGE, "/mutes": { status: 403, body: {} } });
    expect(await fetchAlerts(PROJECT)).toEqual({ kind: "forbidden" });
    lanes({ ...PAGE, "/precision": { status: 503, body: {} } });
    expect(await fetchAlerts(PROJECT)).toEqual({ kind: "unavailable" });
    lanes({ ...PAGE, "/rules": { status: 200, body: { rules: [{ ...ALERT, severity: "red" }] } } });
    expect(await fetchAlerts(PROJECT)).toEqual({ kind: "unavailable" });
  });

  it("asks nothing without a project or a signed-in person", async () => {
    expect(await fetchAlerts("")).toEqual({ kind: "unavailable" });
    identityContext.mockResolvedValue(null);
    expect(await fetchAlerts(PROJECT)).toEqual({ kind: "unavailable" });
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });
});

describe("fetchRunIncidents", () => {
  it("asks for the incidents whose notices showed the run", async () => {
    answer(200, { incidents: [INCIDENT_ROW] });
    expect(await fetchRunIncidents(PROJECT, RUN)).toMatchObject([{ id: INCIDENT }]);
    expect(fetchWithTimeout.mock.calls[0][0]).toBe(
      `http://server/internal/v1/telemetry/incidents?run=${RUN}`,
    );
  });

  // The trace's page goes on without them.
  it("answers none for anything it cannot read, and asks nothing for no run's id", async () => {
    expect(await fetchRunIncidents(PROJECT, "a&run=b")).toEqual([]);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
    answer(403, {});
    expect(await fetchRunIncidents(PROJECT, RUN)).toEqual([]);
    fetchWithTimeout.mockRejectedValue(new Error("timed out"));
    expect(await fetchRunIncidents(PROJECT, RUN)).toEqual([]);
  });
});

describe("fetchIncident", () => {
  beforeEach(() => answer(200, VOTES));

  it("reads one of the project's incidents, with its votes and the person's own", async () => {
    const access = await fetchIncident(PROJECT, INCIDENT);
    expect(access).toMatchObject({
      kind: "ok",
      incident: { rule_name: "Slow calls", useful: 2, noise: 1, yours: "noise" },
    });
    const [url, init] = fetchWithTimeout.mock.calls[0];
    expect(url).toBe(`http://server/internal/v1/telemetry/incidents/${INCIDENT}`);
    expect(init.headers).toMatchObject({ "x-project-id": PROJECT });
  });

  // The id is the path's: one that is not a UUID is the server's 400.
  it("reads a refused id as no such incident, and a 403 as forbidden", async () => {
    answer(404, {});
    expect(await fetchIncident(PROJECT, INCIDENT)).toEqual({ kind: "missing" });
    answer(400, {});
    expect(await fetchIncident(PROJECT, "../rules")).toEqual({ kind: "missing" });
    expect(fetchWithTimeout.mock.calls[1][0]).toBe(
      "http://server/internal/v1/telemetry/incidents/..%2Frules",
    );
    answer(403, {});
    expect(await fetchIncident(PROJECT, INCIDENT)).toEqual({ kind: "forbidden" });
  });

  it("answers an outage for an answer it cannot read, or a server it cannot reach", async () => {
    answer(200, { ...VOTES, yours: "maybe" });
    expect(await fetchIncident(PROJECT, INCIDENT)).toEqual({ kind: "unavailable" });
    answer(503, VOTES);
    expect(await fetchIncident(PROJECT, INCIDENT)).toEqual({ kind: "unavailable" });
    fetchWithTimeout.mockRejectedValue(new Error("timed out"));
    expect(await fetchIncident(PROJECT, INCIDENT)).toEqual({ kind: "unavailable" });
  });

  it("asks nothing without a project or a signed-in person", async () => {
    expect(await fetchIncident("", INCIDENT)).toEqual({ kind: "unavailable" });
    identityContext.mockResolvedValue(null);
    expect(await fetchIncident(PROJECT, INCIDENT)).toEqual({ kind: "unavailable" });
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });
});
