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
  fetchTraceLayouts,
  fetchTraces,
  fetchTraceViews,
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
  models: ["claude-sonnet-5-5"],
  tools: ["search_docs"],
  environments: ["production"],
  releases: ["2026.10.18"],
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
  bars: [
    {
      at: "2026-10-11T02:30:00Z",
      ok: 1,
      error: 0,
      cancelled: 0,
      tokens: 1500,
      duration_p50_ms: 2000,
      duration_p95_ms: 2000,
      time_to_first_token_p50_ms: 400,
      time_to_first_token_p95_ms: 400,
    },
    {
      at: "2026-10-11T03:30:00Z",
      ok: 0,
      error: 0,
      cancelled: 0,
      tokens: 0,
      duration_p50_ms: null,
      duration_p95_ms: null,
      time_to_first_token_p50_ms: null,
      time_to_first_token_p95_ms: null,
    },
  ],
};

const LAYOUT = {
  id: "7c1e0b6f-6a4f-4d2b-9e3a-5c7d8e9f0a1b",
  kind: "layout",
  name: "Review",
  query: {
    depth: "calls",
    open: "failed",
    timeline: true,
    pinned: ["plan"],
    filter: { kind: ["tool_call"] },
    messages: false,
    scores: true,
  },
  columns: ["started", "error_type"],
  created_at: "2026-10-17T00:00:00Z",
  updated_at: "2026-10-17T00:00:00Z",
};

const VIEW = {
  kind: "traces",
  id: "0b6f2c1e-6a4f-4d2b-9e3a-5c7d8e9f0a1b",
  name: "Errors",
  query: { status: ["error"], sort: "duration" },
  columns: ["started", "duration"],
  created_at: "2026-10-11T03:00:00Z",
  updated_at: "2026-10-11T03:00:00Z",
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
};

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
});

describe("fetchTraces", () => {
  it("reads a page of the list from telemetry's lane, as the project, a filter's values given again", async () => {
    answer(200, LIST);
    expect(
      await fetchTraces(PROJECT, { agent: ["nightly-report", "zeta"], sort: "user", limit: 50, page: 2 }),
    ).toEqual({
      kind: "ok",
      list: LIST,
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

  it("reads the project's saved views of the list, and its layouts apart from them", async () => {
    answer(200, { views: [VIEW] });
    expect(await fetchTraceViews(PROJECT)).toEqual({ kind: "ok", views: [VIEW] });
    const [url] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe("http://server/internal/v1/telemetry/views?kind=traces");

    answer(200, { views: [LAYOUT] });
    expect(await fetchTraceLayouts(PROJECT)).toEqual({ kind: "ok", layouts: [LAYOUT] });
    expect(fetchWithTimeout.mock.calls[1]![0]).toBe(
      "http://server/internal/v1/telemetry/views?kind=layout",
    );
    // A layout among the list's views, or a view among the layouts, is an
    // answer this console cannot read.
    answer(200, { views: [LAYOUT] });
    expect(await fetchTraceViews(PROJECT)).toEqual({ kind: "unavailable" });
    answer(200, { views: [{ ...LAYOUT, query: { ...LAYOUT.query, depth: "spans" } }] });
    expect(await fetchTraceLayouts(PROJECT)).toEqual({ kind: "unavailable" });
  });

  it("reads a refusal as forbidden, and a shape it cannot read as unavailable", async () => {
    answer(403, {});
    expect(await fetchTraceFacets(PROJECT, {})).toEqual({ kind: "forbidden" });
    answer(200, { ...HISTOGRAM, bars: [{ at: "x" }] });
    expect(await fetchTraceHistogram(PROJECT, {}, 0)).toEqual({ kind: "unavailable" });
    answer(200, { views: [{ ...VIEW, columns: ["run_id"] }] });
    expect(await fetchTraceViews(PROJECT)).toEqual({ kind: "unavailable" });
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

  it("asks for the list's filters and sort as a file, never its page, or the chosen runs", async () => {
    file(200);
    const result = await traceDownload(
      PROJECT,
      { agent: ["zeta"], sort: "tokens", page: 3, limit: 25 },
      "csv",
      [RUN],
    );
    expect(result).toMatchObject({ kind: "ok", contentType: "text/csv; charset=utf-8" });
    const [url, , timeout] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe(
      `http://server/internal/v1/telemetry/traces/export?agent=zeta&sort=tokens&format=csv&run=${RUN}`,
    );
    expect(timeout).toBeGreaterThan(10_000);
  });

  it("reads a 400 as a refusal of the query, and a 403 as forbidden", async () => {
    file(400);
    expect(await traceDownload(PROJECT, {}, "json", [])).toEqual({ kind: "refused" });
    file(403);
    expect(await traceDownload(PROJECT, {}, "json", [])).toEqual({ kind: "forbidden" });
    file(503);
    expect(await traceDownload(PROJECT, {}, "json", [])).toEqual({ kind: "unavailable" });
  });
});

describe("fetchTrace", () => {
  it("reads one trace's spans by the run's id", async () => {
    answer(200, DETAIL);
    expect(await fetchTrace(PROJECT, RUN)).toEqual({ kind: "ok", trace: DETAIL });
    const [url] = fetchWithTimeout.mock.calls[0]!;
    expect(url).toBe(`http://server/internal/v1/telemetry/traces/${RUN}`);
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
