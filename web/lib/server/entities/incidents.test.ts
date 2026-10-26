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
import { fetchIncident } from "./incidents";

const PROJECT = "project_1";
const INCIDENT = "5d4f5c68-0000-4000-8000-000000000001";
const ANSWER = {
  id: INCIDENT,
  rule_id: "7a1c0e52-0000-4000-8000-000000000002",
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
  useful: 2,
  noise: 1,
  yours: "noise",
};

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
  answer(200, ANSWER);
});

describe("fetchIncident", () => {
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
    answer(200, { ...ANSWER, yours: "maybe" });
    expect(await fetchIncident(PROJECT, INCIDENT)).toEqual({ kind: "unavailable" });
    answer(503, ANSWER);
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
