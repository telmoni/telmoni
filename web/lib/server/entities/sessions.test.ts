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
import { fetchSessionFacets, fetchSessions, sessionDownload } from "./sessions";

const PROJECT = "project_1";

const SESSION = {
  session_id: "9f86d081884c7d659a2feaa0c55ad015",
  user_id: "",
  agents: ["support-bot", "triage"],
  traces: 3,
  failed_traces: 1,
  input_tokens: 300,
  output_tokens: 60,
  spend: 0.875,
  duration_ms: 3000,
  first_seen: "2026-10-11T03:00:00Z",
  last_seen: "2026-10-11T03:20:01Z",
  last_ended: true,
  last_status: "ok",
  tags: ["repo:telmoni"],
};
const LIST = {
  sessions: [SESSION],
  days: 7,
  limit: 50,
  page: 1,
  total: 1,
  more: false,
  sort: "last_seen",
  dir: "desc",
};

function answer(status: number, body: unknown) {
  fetchWithTimeout.mockResolvedValue({ ok: status < 400, status, json: async () => body });
}

const asked = () => String(fetchWithTimeout.mock.calls[0]?.[0]);

beforeEach(() => {
  vi.clearAllMocks();
  identityContext.mockResolvedValue({ userId: "user_1", organizationId: "org_1" });
});

describe("the sessions lanes", () => {
  it("reads a page of the list as the project, the trace list's filters among its query", async () => {
    const resolved = { name: "resolved", data_type: "boolean", mean: 1, count: 1, categories: [] };
    const QUIET = { ...SESSION, session_id: "fcde2b2edba56bf408601fb721fe9b5c" };
    answer(200, { ...LIST, sessions: [SESSION, QUIET], scores: { [SESSION.session_id]: [resolved] } });
    // Each session's own scores are put on it, none where the map names none.
    expect(await fetchSessions(PROJECT, { agent: ["triage"], sort: "spend", days: 30, limit: 50 })).toEqual({
      kind: "ok",
      list: {
        ...LIST,
        sessions: [
          { ...SESSION, scores: [resolved] },
          { ...QUIET, scores: [] },
        ],
      },
    });
    expect(asked()).toBe("http://server/internal/v1/telemetry/sessions?agent=triage&sort=spend&days=30&limit=50");
    expect(fetchWithTimeout.mock.calls[0]?.[1].headers).toMatchObject({ "x-project-id": PROJECT });
  });

  it("refuses a row that is not a session's, rather than drawing half of one", async () => {
    answer(200, { ...LIST, sessions: [{ ...SESSION, last_status: "paused" }] });
    expect(await fetchSessions(PROJECT, {})).toEqual({ kind: "unavailable" });
    answer(200, { ...LIST, sort: "session" });
    expect(await fetchSessions(PROJECT, {})).toEqual({ kind: "unavailable" });
  });

  it("asks the facets for the filters and the window alone, and the download for its sort and shape", async () => {
    answer(403, {});
    expect(await fetchSessionFacets(PROJECT, { status: ["error"], sort: "spend", page: 2, days: 1 })).toEqual({
      kind: "forbidden",
    });
    expect(asked()).toBe("http://server/internal/v1/telemetry/sessions/facets?status=error&days=1");

    fetchWithTimeout.mockClear();
    fetchWithTimeout.mockResolvedValue({ ok: false, status: 400, json: async () => ({}) });
    expect(await sessionDownload(PROJECT, { tag: ["repo:telmoni"], sort: "spend", page: 2 }, "json")).toEqual({
      kind: "refused",
    });
    expect(asked()).toBe(
      "http://server/internal/v1/telemetry/sessions/export?tag=repo%3Atelmoni&sort=spend&format=json",
    );
  });
});
