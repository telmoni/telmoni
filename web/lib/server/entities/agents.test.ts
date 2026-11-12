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
import { fetchAgents } from "./agents";

const PROJECT = "project_1";

const AGENT = {
  agent: " padded ",
  traces: 3,
  failed_traces: 1,
  faults: { rate_limited: 2 },
  spend: 0.6,
  last_trace_id: "5d4f5c68000040008000000000000001",
  last_started_at: "2026-10-11T03:00:00Z",
  last_ended: true,
  last_status: "ok",
  last_spend: 0.2,
  usual_spend: 0.2,
  models: ["model-a"],
  prompt_versions: [],
};

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

const asked = () => String(fetchWithTimeout.mock.calls[0]?.[0]);

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
});

describe("the agents lane", () => {
  it("reads every agent over the server's own days, or one as it is named over those asked", async () => {
    answer(200, { days: 14, agents: [AGENT], changes: [] });
    const every = await fetchAgents(PROJECT);
    expect(every.kind === "ok" && every.value.agents[0]?.faults).toEqual({ rate_limited: 2 });
    expect(asked()).toBe("http://server/internal/v1/telemetry/agents");

    fetchWithTimeout.mockClear();
    answer(200, { days: 30, agents: [AGENT], changes: [] });
    await fetchAgents(PROJECT, " padded ", 30);
    // Its spaces kept: a name is asked for whole.
    expect(asked()).toBe("http://server/internal/v1/telemetry/agents?agent=+padded+&days=30");
  });

  it("refuses a fault the contract has none of", async () => {
    answer(200, { days: 14, agents: [{ ...AGENT, faults: { melted: 1 } }], changes: [] });
    expect(await fetchAgents(PROJECT)).toEqual({ kind: "unavailable" });
  });
});
