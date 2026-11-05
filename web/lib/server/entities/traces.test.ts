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
import {
  fetchTrace,
  fetchTraceFacets,
  fetchTraceHistogram,
  fetchTraces,
  traceDownload,
} from "./traces";

const PROJECT = "project_1";
const RUN = "5d4f5c68-0000-4000-8000-000000000001";

const TRACE = {
  run_id: RUN,
  agent: "nightly-report",
  user_id: "9f86d081884c7d659a2feaa0c55ad015",
  session_id: "conv-1",
  ended: true,
  status: "ok",
  error_type: "",
  started_at: "2026-10-11T03:00:00Z",
  ended_at: "2026-10-11T03:00:02Z",
  duration_ms: 2000,
  time_to_first_token_ms: 400,
  spans: 4,
  steps: 1,
  model_calls: 1,
  tool_calls: 1,
  errors: 1,
  input_tokens: 1200,
  output_tokens: 300,
  cache_read_input_tokens: 100,
  cache_write_input_tokens: 50,
  spend: 0.25,
  models: ["claude-sonnet-5-5"],
  tools: ["search_docs"],
  environments: ["production"],
  releases: ["2026.10.18"],
  tags: ["team:search"],
};
const LIST = {
  traces: [TRACE],
  days: 7,
  limit: 50,
  page: 1,
  total: 1,
  more: false,
  sort: "started",
  dir: "desc",
};
// What the console makes of it: each run's own scores put on its trace.
const LISTED = { ...LIST, traces: [{ ...TRACE, scores: [] }] };

const FACET = { values: [{ value: "nightly-report", count: 1 }], more: false };
const FACETS = {
  days: 7,
  agent: FACET,
  user: { values: [], more: false },
  session: { values: [], more: false },
  status: { values: [{ value: "ok", count: 1 }], more: false },
  model: { values: [], more: false },
  tool: { values: [], more: false },
  environment: { values: [{ value: "production", count: 1 }], more: false },
  release: { values: [], more: false },
  tag: { values: [{ value: "team:search", count: 1 }], more: false },
  prompt: { values: [], more: false },
};

const HISTOGRAM = {
  days: 1,
  bucket_seconds: 3600,
  utc_offset: 330,
  score: null,
  bars: [
    {
      at: "2026-10-11T02:30:00Z",
      ok: 1,
      error: 0,
      cancelled: 0,
      tokens: 1500,
      spend: 0.25,
      duration_p50_ms: 2000,
      duration_p95_ms: 2000,
      time_to_first_token_p50_ms: 400,
      time_to_first_token_p95_ms: 400,
      score_mean: null,
    },
    {
      at: "2026-10-11T03:30:00Z",
      ok: 0,
      error: 0,
      cancelled: 0,
      tokens: 0,
      spend: 0,
      duration_p50_ms: null,
      duration_p95_ms: null,
      time_to_first_token_p50_ms: null,
      time_to_first_token_p95_ms: null,
      score_mean: null,
    },
  ],
};

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
  cost: null,
  time_to_first_token: null,
  user_id: "9f86d081884c7d659a2feaa0c55ad015",
  session_id: "conv-1",
  environment: "production",
  release: "2026.10.18",
  tags: { team: "search" },
  sample_rate: 1,
  attributes: {},
  units: 1,
  received_at: "2026-10-11T03:00:02Z",
};
const DETAIL = {
  run_id: RUN,
  spans: [SPAN],
  truncated: false,
  spend: { spent: 0.0028, unpriced: [] },
  content: [],
};

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
});

describe("fetchTraces", () => {
  it("puts each run's own scores on its trace", async () => {
    const helpful = { name: "helpful", data_type: "number", mean: 0.8, count: 1, categories: [] };
    answer(200, { ...LIST, scores: { [TRACE.run_id]: [helpful] } });
    const read = await fetchTraces(PROJECT, {});
    expect(read.kind === "ok" && read.list.traces[0]?.scores).toEqual([helpful]);
  });

  it("reads a page of the list from telemetry's lane, as the project, a filter's values given again", async () => {
    answer(200, LIST);
    expect(
      await fetchTraces(PROJECT, { agent: ["nightly-report", "zeta"], sort: "user", limit: 50, page: 2 }),
    ).toEqual({
      kind: "ok",
      list: LISTED,
    });
    const [url, init] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe(
      "http://server/internal/v1/telemetry/traces?agent=nightly-report&agent=zeta&sort=user&limit=50&page=2",
    );
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

  it("asks for the list sorted by a tag's key, and reads the sort it answers, but none naming no key", async () => {
    answer(200, { ...LIST, sort: "tag:repo", dir: "asc" });
    expect(await fetchTraces(PROJECT, { sort: "tag:repo" })).toEqual({
      kind: "ok",
      list: { ...LISTED, sort: "tag:repo", dir: "asc" },
    });
    expect(fetchWithTimeout.mock.calls[0]![0]).toBe(
      "http://server/internal/v1/telemetry/traces?sort=tag%3Arepo",
    );
    answer(200, { ...LIST, sort: "tag:" });
    expect(await fetchTraces(PROJECT, {})).toEqual({ kind: "unavailable" });
  });

  it("asks nothing without a project or a signed-in person", async () => {
    expect(await fetchTraces("", {})).toEqual({ kind: "unavailable" });
    identityContext.mockResolvedValue(null);
    expect(await fetchTraces(PROJECT, {})).toEqual({ kind: "unavailable" });
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });
});

describe("the reads beside the list", () => {
  // Each of these lanes refuses a parameter it has no use for, so each is
  // sent the filters and the window alone.
  const QUERY = { agent: ["zeta"], status: ["error" as const], sort: "tokens" as const, page: 3, limit: 25, days: 30 };

  it("reads each filter's values with the filters and the window alone", async () => {
    answer(200, FACETS);
    expect(await fetchTraceFacets(PROJECT, QUERY)).toEqual({ kind: "ok", facets: FACETS });
    const [url] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe("http://server/internal/v1/telemetry/traces/facets?agent=zeta&status=error&days=30");
  });

  it("reads the histogram on the reader's clock, its empty bars with no durations", async () => {
    answer(200, HISTOGRAM);
    expect(await fetchTraceHistogram(PROJECT, QUERY, 330)).toEqual({ kind: "ok", histogram: HISTOGRAM });
    const [url] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe(
      "http://server/internal/v1/telemetry/traces/histogram?agent=zeta&status=error&days=30&utc_offset=330",
    );
  });

  it("reads a refusal as forbidden, and a shape it cannot read as unavailable", async () => {
    answer(403, {});
    expect(await fetchTraceFacets(PROJECT, {})).toEqual({ kind: "forbidden" });
    answer(200, { ...HISTOGRAM, bars: [{ at: "x" }] });
    expect(await fetchTraceHistogram(PROJECT, {}, 0)).toEqual({ kind: "unavailable" });
  });
});

describe("traceDownload", () => {
  function file(status: number, contentType = "text/csv; charset=utf-8") {
    fetchWithTimeout.mockResolvedValue({
      ok: status < 400,
      status,
      headers: new Headers({ "content-type": contentType }),
      arrayBuffer: async () => new TextEncoder().encode("run_id\r\n").buffer,
    });
  }

  it("asks for the list's filters and sort as a file, never its page", async () => {
    file(200);
    const result = await traceDownload(
      PROJECT,
      { agent: ["zeta"], sort: "tokens", page: 3, limit: 25 },
      "csv",
    );
    expect(result).toMatchObject({ kind: "ok", contentType: "text/csv; charset=utf-8" });
    const [url, , timeout] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe(
      "http://server/internal/v1/telemetry/traces/export?agent=zeta&sort=tokens&format=csv",
    );
    expect(timeout).toBeGreaterThan(10_000);
  });

  it("reads a 400 as a refusal of the query, and a 403 as forbidden", async () => {
    file(400);
    expect(await traceDownload(PROJECT, {}, "json")).toEqual({ kind: "refused" });
    file(403);
    expect(await traceDownload(PROJECT, {}, "json")).toEqual({ kind: "forbidden" });
    file(503);
    expect(await traceDownload(PROJECT, {}, "json")).toEqual({ kind: "unavailable" });
  });
});

describe("fetchTrace", () => {
  it("reads one trace's spans by the run's id, its content only where asked", async () => {
    answer(200, DETAIL);
    expect(await fetchTrace(PROJECT, RUN, false)).toEqual({ kind: "ok", trace: DETAIL });
    expect(fetchWithTimeout.mock.calls[0]![0]).toBe(`http://server/internal/v1/telemetry/traces/${RUN}`);
    answer(200, DETAIL);
    await fetchTrace(PROJECT, RUN, true);
    expect(fetchWithTimeout.mock.calls.at(-1)![0]).toBe(
      `http://server/internal/v1/telemetry/traces/${RUN}?content=true`,
    );
  });

  // A run the project does not hold and an id that is no run id read the
  // same: the page says there is no such trace here.
  it("reads a 404 and a 400 as missing", async () => {
    answer(404, {});
    expect(await fetchTrace(PROJECT, RUN, false)).toEqual({ kind: "missing" });
    answer(400, {});
    expect(await fetchTrace(PROJECT, "not-a-run", false)).toEqual({ kind: "missing" });
  });

  it("reads a refusal as forbidden, and anything else as unavailable", async () => {
    answer(403, {});
    expect(await fetchTrace(PROJECT, RUN, false)).toEqual({ kind: "forbidden" });
    answer(200, { run_id: RUN, spans: [{ kind: "run" }], truncated: false });
    expect(await fetchTrace(PROJECT, RUN, false)).toEqual({ kind: "unavailable" });
    fetchWithTimeout.mockRejectedValue(new Error("timed out"));
    expect(await fetchTrace(PROJECT, RUN, false)).toEqual({ kind: "unavailable" });
  });
});
