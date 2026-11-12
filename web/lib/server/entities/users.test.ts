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
import { fetchUserFacets, fetchUsers, userDownload } from "./users";

const PROJECT = "project_1";

const USER = {
  user_id: "2c26b46b68ffc68ff99b453c1d304134",
  sessions: 2,
  agents: ["support-bot", "triage"],
  traces: 4,
  failed_traces: 1,
  input_tokens: 400,
  output_tokens: 80,
  spend: 1,
  duration_ms: 4000,
  first_seen: "2026-10-11T03:00:00Z",
  last_seen: "2026-10-11T03:22:01Z",
  last_ended: true,
  last_status: "ok",
  tags: ["repo:telmoni"],
};
const LIST = {
  users: [USER],
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

describe("the users lanes", () => {
  it("reads a page of the list as the project, the trace list's filters among its query", async () => {
    answer(200, LIST);
    expect(await fetchUsers(PROJECT, { agent: ["triage"], sort: "sessions", days: 30, limit: 50 })).toEqual({
      kind: "ok",
      list: LIST,
    });
    expect(asked()).toBe("http://server/internal/v1/telemetry/users?agent=triage&sort=sessions&days=30&limit=50");
    expect(fetchWithTimeout.mock.calls[0]?.[1].headers).toMatchObject({ "x-project-id": PROJECT });
  });

  it("refuses a row that is not a user's, rather than drawing half of one", async () => {
    answer(200, { ...LIST, users: [{ ...USER, sessions: "2" }] });
    expect(await fetchUsers(PROJECT, {})).toEqual({ kind: "unavailable" });
    answer(200, { ...LIST, users: [{ ...USER, last_status: "paused" }] });
    expect(await fetchUsers(PROJECT, {})).toEqual({ kind: "unavailable" });
    // A sort of the trace list's is none of this list's.
    answer(200, { ...LIST, sort: "started" });
    expect(await fetchUsers(PROJECT, {})).toEqual({ kind: "unavailable" });
  });

  it("asks the facets for the filters and the window alone, and the download for its sort and shape", async () => {
    answer(403, {});
    expect(await fetchUserFacets(PROJECT, { status: ["error"], sort: "spend", page: 2, days: 1 })).toEqual({
      kind: "forbidden",
    });
    expect(asked()).toBe("http://server/internal/v1/telemetry/users/facets?status=error&days=1");

    fetchWithTimeout.mockClear();
    fetchWithTimeout.mockResolvedValue({ ok: false, status: 400, json: async () => ({}) });
    expect(await userDownload(PROJECT, { tag: ["repo:telmoni"], sort: "spend", page: 2 }, "json")).toEqual({
      kind: "refused",
    });
    expect(asked()).toBe("http://server/internal/v1/telemetry/users/export?tag=repo%3Atelmoni&sort=spend&format=json");
  });
});
