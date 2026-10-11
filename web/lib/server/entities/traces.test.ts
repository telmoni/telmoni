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
import { fetchTrace, fetchTraces } from "./traces";

const PROJECT = "project_1";
const RUN = "5d4f5c68-0000-4000-8000-000000000001";

const TRACE = {
  run_id: RUN,
  agent: "nightly-report",
  user_id: "9f86d081884c7d659a2feaa0c55ad015",
  conversation_id: "conv-1",
  ended: true,
  status: "ok",
  started_at: "2026-10-11T03:00:00Z",
  ended_at: "2026-10-11T03:00:02Z",
  duration_ms: 2000,
  spans: 4,
  steps: 1,
  model_calls: 1,
  tool_calls: 1,
  errors: 1,
  input_tokens: 1200,
  output_tokens: 300,
  cache_read_input_tokens: 100,
  cache_write_input_tokens: 50,
  models: ["claude-sonnet-5-5"],
  tools: ["search_docs"],
};
const LIST = { traces: [TRACE], days: 7, limit: 100, more: false, sort: "started", dir: "desc" };

const SPAN = {
  span_id: "0000000000000001",
  parent_span_id: "",
  trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
  agent: "nightly-report",
  kind: "run",
  name: "nightly-report",
  started_at: "2026-10-11T03:00:00Z",
  ended_at: "2026-10-11T03:00:02Z",
  status: "ok",
  error_type: "",
  model: "",
  provider: "",
  input_tokens: 0,
  output_tokens: 0,
  cache_read_input_tokens: 0,
  cache_write_input_tokens: 0,
  user_id: "9f86d081884c7d659a2feaa0c55ad015",
  conversation_id: "conv-1",
  tags: { team: "search" },
  bound_stall_after_s: null,
  bound_max_steps: 40,
  bound_max_cost_usd: null,
  sample_rate: 1,
  attributes: {},
  units: 1,
  received_at: "2026-10-11T03:00:02Z",
};
const DETAIL = { run_id: RUN, spans: [SPAN], truncated: false };

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
});

describe("fetchTraces", () => {
  it("reads the list from telemetry's lane, as the project, with the query", async () => {
    answer(200, LIST);
    expect(await fetchTraces(PROJECT, { agent: "nightly-report", sort: "user" })).toEqual({
      kind: "ok",
      list: LIST,
    });
    const [url, init] = fetchWithTimeout.mock.calls[0];
    expect(url).toBe("http://server/internal/telemetry/traces?agent=nightly-report&sort=user");
    expect(init.headers).toMatchObject({ "x-project-id": PROJECT });
  });

  it("reads a refusal as forbidden, and anything else as unavailable", async () => {
    answer(403, {});
    expect(await fetchTraces(PROJECT, {})).toEqual({ kind: "forbidden" });
    answer(503, {});
    expect(await fetchTraces(PROJECT, {})).toEqual({ kind: "unavailable" });
    answer(200, { traces: "none" });
    expect(await fetchTraces(PROJECT, {})).toEqual({ kind: "unavailable" });
    fetchWithTimeout.mockRejectedValue(new Error("timed out"));
    expect(await fetchTraces(PROJECT, {})).toEqual({ kind: "unavailable" });
  });

  it("asks nothing without a project or a signed-in person", async () => {
    expect(await fetchTraces("", {})).toEqual({ kind: "unavailable" });
    identityContext.mockResolvedValue(null);
    expect(await fetchTraces(PROJECT, {})).toEqual({ kind: "unavailable" });
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });
});

describe("fetchTrace", () => {
  it("reads one trace's spans by the run's id", async () => {
    answer(200, DETAIL);
    expect(await fetchTrace(PROJECT, RUN)).toEqual({ kind: "ok", trace: DETAIL });
    const [url] = fetchWithTimeout.mock.calls[0];
    expect(url).toBe(`http://server/internal/telemetry/traces/${RUN}`);
  });

  // A run the project does not hold and an id that is no run id read the
  // same: the page says there is no such trace here.
  it("reads a 404 and a 400 as missing", async () => {
    answer(404, {});
    expect(await fetchTrace(PROJECT, RUN)).toEqual({ kind: "missing" });
    answer(400, {});
    expect(await fetchTrace(PROJECT, "not-a-run")).toEqual({ kind: "missing" });
  });

  it("reads a refusal as forbidden, and anything else as unavailable", async () => {
    answer(403, {});
    expect(await fetchTrace(PROJECT, RUN)).toEqual({ kind: "forbidden" });
    answer(200, { run_id: RUN, spans: [{ kind: "run" }], truncated: false });
    expect(await fetchTrace(PROJECT, RUN)).toEqual({ kind: "unavailable" });
    fetchWithTimeout.mockRejectedValue(new Error("timed out"));
    expect(await fetchTrace(PROJECT, RUN)).toEqual({ kind: "unavailable" });
  });
});
