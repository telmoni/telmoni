import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "a view by that name exists" })),
}));
vi.mock("@/lib/env", () => ({
  env: { SERVER_URL: "http://telemetry.test" },
}));
vi.mock("@/lib/server/identity", () => ({
  SWITCHED_ORGANIZATION: "switched",
  unplacedOrganization: async () => "unplaced",
}));
vi.mock("@/lib/server/entities/identity-context", () => ({
  identityContext: async () => ({
    userId: "user_1",
    organizationId: "org_1",
    role: "admin",
    accessToken: "at_1",
  }),
  sessionHeaders: () => ({ authorization: "Bearer at_1" }),
}));
const mockRefresh = vi.fn();
vi.mock("next/cache", () => ({
  refresh: (...a: unknown[]) => mockRefresh(...a),
}));
vi.mock("next/navigation", () => ({
  redirect: (to: string) => {
    throw new Error(`REDIRECT:${to}`);
  },
}));
// The page the action was posted from, whose path and query the proxy hands
// on: the list on a filter and a window, opened from a saved view.
const posted = vi.hoisted(() => ({ path: "" }));
vi.mock("next/headers", () => ({
  headers: async () => new Headers({ "x-telmoni-path": posted.path }),
}));

import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { getServerSession } from "@/lib/server/session";

import {
  deleteTraceAction,
  deleteTraceViewAction,
  eraseEndUserAction,
  saveTraceViewAction,
  setSessionLayoutAction,
  setTraceTagsAction,
  tagTracesAction,
  updateTraceViewAction,
} from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);
const PROJECT = "project_1";
const OPEN = "11111111-1111-4111-8111-111111111111";
const OTHER = "22222222-2222-4222-8222-222222222222";
const NEW = "33333333-3333-4333-8333-333333333333";
const LIST = `/acme/web/traces?agent=support&days=7&view=${OPEN}`;

const input = {
  name: "Support, a week",
  kind: "traces",
  query: { agent: ["support"], days: 7 },
  columns: [],
};
const view = (id: string) =>
  new Response(
    JSON.stringify({
      id,
      kind: "traces",
      name: "Support, a week",
      query: { agent: ["support"], days: 7 },
      columns: [],
      created_at: "2026-10-12T00:00:00Z",
      updated_at: "2026-10-12T00:00:00Z",
    }),
    { status: 201 },
  );
const layoutInput = {
  name: "Review",
  kind: "layout",
  query: {
    depth: "calls",
    open: "failed",
    timeline: true,
    graph: false,
    pinned: ["plan"],
    filter: { kind: [], status: [], name: [] },
    messages: false,
    scores: true,
  },
  columns: ["started", "error_type"],
};
const layout = (id: string) =>
  new Response(
    JSON.stringify({
      id,
      kind: "layout",
      name: "Review",
      query: layoutInput.query,
      columns: layoutInput.columns,
      created_at: "2026-10-17T00:00:00Z",
      updated_at: "2026-10-17T00:00:00Z",
    }),
    { status: 201 },
  );

beforeEach(() => {
  vi.clearAllMocks();
  posted.path = LIST;
  vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1", accessToken: "at_1" } as never);
});

describe("saving a new view", () => {
  // The list opens on the view just saved, its menu listing it: the page it
  // was saved from, with the view named, drawn with the answer — not drawn
  // again here and then once more where the browser was sent after.
  it("takes the page to the list opened on it, everything else the list held kept", async () => {
    fetchMock.mockResolvedValue(view(NEW));
    await expect(saveTraceViewAction(PROJECT, input)).rejects.toThrow(
      `REDIRECT:/acme/web/traces?agent=support&days=7&view=${NEW}`,
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "http://telemetry.test/internal/v1/telemetry/views",
      expect.objectContaining({ method: "POST" }),
    );
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("stays, saying why, when the lane refuses it", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 409 }));
    expect(await saveTraceViewAction(PROJECT, input)).toEqual({
      ok: false,
      error: "a view by that name exists",
    });
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  // A layout saved from a session's page opens the page on it.
  it("takes a session's page to the layout saved", async () => {
    posted.path = "/acme/web/sessions/conv-1?days=30";
    fetchMock.mockResolvedValue(layout(NEW));
    await expect(saveTraceViewAction(PROJECT, layoutInput)).rejects.toThrow(
      `REDIRECT:/acme/web/sessions/conv-1?days=30&view=${NEW}`,
    );
    const [, init] = fetchMock.mock.calls[0]!;
    expect(JSON.parse(init!.body as string)).toEqual(layoutInput);
  });

  it("refuses a layout whose options are not ones the page reads before asking the lane", async () => {
    expect(
      await saveTraceViewAction(PROJECT, { ...layoutInput, query: { ...layoutInput.query, depth: "spans" } }),
    ).toEqual({ ok: false, error: "That view could not be read." });
    expect(
      await saveTraceViewAction(PROJECT, { ...layoutInput, kind: "traces" }),
    ).toEqual({ ok: false, error: "That view could not be read." });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // A dashboard saved from the dashboards page opens the page on it, its
  // charts and window kept.
  it("takes the dashboards page to the dashboard saved, and refuses one with columns", async () => {
    const dashboardInput = {
      name: "Team spend",
      kind: "dashboard",
      query: { charts: [{ measure: "spend", group: "tag", tag: "team" }] },
      columns: [],
    };
    posted.path = "/acme/web/dashboards?days=30&chart=spend%3Atag%3Ateam";
    fetchMock.mockResolvedValue(
      new Response(
        JSON.stringify({
          id: NEW,
          ...dashboardInput,
          created_at: "2026-11-02T00:00:00Z",
          updated_at: "2026-11-02T00:00:00Z",
        }),
        { status: 201 },
      ),
    );
    await expect(saveTraceViewAction(PROJECT, dashboardInput)).rejects.toThrow(
      `REDIRECT:/acme/web/dashboards?days=30&chart=spend%3Atag%3Ateam&view=${NEW}`,
    );
    fetchMock.mockClear();
    expect(await saveTraceViewAction(PROJECT, { ...dashboardInput, columns: ["agent"] })).toEqual({
      ok: false,
      error: "That view could not be read.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // Saved all the same, so the menu has to list it: the page is drawn again.
  it("draws the list again when the lane's answer cannot be read", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 201 }));
    expect(await saveTraceViewAction(PROJECT, input)).toEqual({
      ok: false,
      error: "Unexpected response from telemetry.",
    });
    expect(mockRefresh).toHaveBeenCalledOnce();
  });

  it("refuses what is not a view before asking the lane", async () => {
    expect(await saveTraceViewAction(PROJECT, { name: 3 })).toEqual({
      ok: false,
      error: "That view could not be read.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("tells a session that ran out to sign in again, asking the lane nothing", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null);
    expect(await saveTraceViewAction(PROJECT, input)).toEqual({
      ok: false,
      error: "Your session expired — sign in again.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("saving over a view", () => {
  it("draws the list again where it is, its menu with the change", async () => {
    fetchMock.mockResolvedValue(view(OPEN));
    expect(await updateTraceViewAction(PROJECT, OPEN, input)).toMatchObject({ ok: true });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://telemetry.test/internal/v1/telemetry/views/${OPEN}`,
      expect.objectContaining({ method: "PUT" }),
    );
    expect(mockRefresh).toHaveBeenCalledOnce();
  });
});

describe("deleting a view", () => {
  // The page was opened from it: back to the list it showed, without it.
  it("takes the page opened from it back to the list it showed", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    await expect(deleteTraceViewAction(PROJECT, OPEN)).rejects.toThrow(
      "REDIRECT:/acme/web/traces?agent=support&days=7",
    );
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("draws any other page again where it is, its menu without the view", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await deleteTraceViewAction(PROJECT, OTHER)).toEqual({ ok: true });
    expect(mockRefresh).toHaveBeenCalledOnce();
  });

  it("refuses what is not a view's id before asking the lane", async () => {
    expect(await deleteTraceViewAction(PROJECT, "../views")).toEqual({
      ok: false,
      error: "Invalid view id.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("setting what the project opens in", () => {
  const setting = (body: unknown, status = 200) =>
    new Response(JSON.stringify(body), { status });

  it("sends a saved layout, or null for the default, and draws the page again with the answer", async () => {
    fetchMock.mockResolvedValue(setting({ view_id: OPEN }));
    expect(await setSessionLayoutAction(PROJECT, { view_id: OPEN })).toEqual({
      ok: true,
      setting: { view_id: OPEN },
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://telemetry.test/internal/v1/telemetry/session-layout",
      expect.objectContaining({ method: "PUT", body: JSON.stringify({ view_id: OPEN }) }),
    );
    expect(mockRefresh).toHaveBeenCalledOnce();

    fetchMock.mockResolvedValue(setting({ view_id: null }));
    expect(await setSessionLayoutAction(PROJECT, { view_id: null })).toEqual({
      ok: true,
      setting: { view_id: null },
    });
  });

  it("refuses what is no layout before asking the lane, and says why the lane refused", async () => {
    for (const input of [{}, { layout: "complex" }, { view_id: "mine" }, { view_id: OPEN, layout: "simple" }]) {
      expect(await setSessionLayoutAction(PROJECT, input)).toEqual({
        ok: false,
        error: "That layout could not be read.",
      });
    }
    expect(fetchMock).not.toHaveBeenCalled();
    fetchMock.mockResolvedValue(new Response(null, { status: 404 }));
    expect(await setSessionLayoutAction(PROJECT, { view_id: OTHER })).toEqual({
      ok: false,
      error: "a view by that name exists",
    });
    expect(mockRefresh).not.toHaveBeenCalled();
  });
});

describe("deleting a trace", () => {
  it("goes back to the list, its query dropped, once the lane deleted it", async () => {
    posted.path = `/acme/web/traces/${OPEN}?span=abc`;
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ run_id: OPEN, spans: 3, fields: 0 }), { status: 200 }),
    );
    await expect(deleteTraceAction(PROJECT, OPEN)).rejects.toThrow("REDIRECT:/acme/web/traces");
    expect(fetchMock).toHaveBeenCalledWith(
      `http://telemetry.test/internal/v1/telemetry/traces/${OPEN}`,
      expect.objectContaining({ method: "DELETE" }),
      60_000,
    );
  });

  it("refuses what is no run's id before asking the lane, and says why the lane refused", async () => {
    expect(await deleteTraceAction(PROJECT, "../views")).toEqual({
      ok: false,
      error: "Invalid trace id.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
    fetchMock.mockResolvedValue(new Response(null, { status: 404 }));
    expect((await deleteTraceAction(PROJECT, OPEN)).ok).toBe(false);
  });
});

describe("erasing an end user", () => {
  it("sends the id in the body, never the URL, and answers how many traces went", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ traces: 2, spans: 9, fields: 4 }), { status: 200 }),
    );
    expect(await eraseEndUserAction(PROJECT, "acme 42")).toEqual({ ok: true, traces: 2 });
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe("http://telemetry.test/internal/v1/telemetry/users/erase");
    expect(url).not.toContain("acme");
    expect(init).toEqual(
      expect.objectContaining({ method: "POST", body: JSON.stringify({ user_id: "acme 42" }) }),
    );
  });

  it("counts an id's length in characters, as ingest does, before asking the lane", async () => {
    fetchMock.mockResolvedValue(new Response(JSON.stringify({ traces: 0 }), { status: 200 }));
    expect((await eraseEndUserAction(PROJECT, "😀".repeat(128))).ok).toBe(true);
    for (const id of ["", "x".repeat(129), 42]) {
      expect(await eraseEndUserAction(PROJECT, id)).toEqual({
        ok: false,
        error: "An end user's id is 1 to 128 characters.",
      });
    }
    expect(fetchMock).toHaveBeenCalledOnce();
  });
});

// The run's tags after a change, as the lane answers them.
const tagged = (tags: Record<string, string>) =>
  new Response(JSON.stringify({ run_id: OPEN, tags }), { status: 200 });
// A refusal as the server words it, read by the console's own reader.
async function refusedWith(status: number, type: string, title: string, detail: string) {
  const real = await vi.importActual<typeof import("@/lib/api/fetch")>("@/lib/api/fetch");
  vi.mocked(extractProblem).mockImplementationOnce(real.extractProblem);
  return new Response(JSON.stringify({ type, title, status, detail }), {
    status,
    headers: { "content-type": "application/problem+json" },
  });
}

describe("changing a run's tags", () => {
  it("sends the change to the run's lane as the person, and draws the page again", async () => {
    fetchMock.mockResolvedValue(tagged({ repo: "telmoni" }));
    expect(
      await setTraceTagsAction(PROJECT, OPEN, { add: { repo: "telmoni" }, remove: ["team"] }),
    ).toEqual({ ok: true, tags: { repo: "telmoni" } });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://telemetry.test/internal/v1/telemetry/traces/${OPEN}/tags`,
      {
        method: "PATCH",
        headers: { authorization: "Bearer at_1", "content-type": "application/json" },
        body: JSON.stringify({ add: { repo: "telmoni" }, remove: ["team"] }),
      },
      30_000,
    );
    expect(mockRefresh).toHaveBeenCalledOnce();
  });

  it("says in the server's words why a tag past its cap was refused, and a run the project does not hold", async () => {
    fetchMock.mockResolvedValueOnce(
      await refusedWith(400, "/errors/telemetry/tags-past-cap", "tags past their cap", "a span carries up to 32 tags"),
    );
    expect(await setTraceTagsAction(PROJECT, OPEN, { add: { repo: "telmoni" } })).toEqual({
      ok: false,
      error: "tags past their cap: a span carries up to 32 tags",
    });
    fetchMock.mockResolvedValueOnce(
      await refusedWith(404, "/errors/auth/not-found", "not found", "no trace with that run id in this project"),
    );
    expect(await setTraceTagsAction(PROJECT, OPEN, { remove: ["repo"] })).toEqual({
      ok: false,
      error: "not found: no trace with that run id in this project",
    });
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("refuses what is no run's id, or no change of tags, before asking the lane", async () => {
    expect(await setTraceTagsAction(PROJECT, "../views", { remove: ["repo"] })).toEqual({
      ok: false,
      error: "Invalid trace id.",
    });
    for (const change of [{ add: { repo: 7 } }, { remove: "repo" }, { rename: { a: "b" } }, null]) {
      expect(await setTraceTagsAction(PROJECT, OPEN, change)).toEqual({
        ok: false,
        error: "That change to the tags could not be read.",
      });
    }
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("tagging every trace a session's page shows", () => {
  it("sets the tag on each run in turn, the person admitted once, and says how many took it", async () => {
    fetchMock.mockImplementation(async () => tagged({ team: "core" }));
    expect(await tagTracesAction(PROJECT, [OPEN, OTHER, NEW], { key: "team", value: "core" })).toEqual({
      ok: true,
      tagged: 3,
    });
    expect(fetchMock.mock.calls.map(([url]) => url)).toEqual(
      [OPEN, OTHER, NEW].map((run) => `http://telemetry.test/internal/v1/telemetry/traces/${run}/tags`),
    );
    for (const [, init] of fetchMock.mock.calls) {
      expect(init).toEqual(
        expect.objectContaining({ method: "PATCH", body: JSON.stringify({ add: { team: "core" } }) }),
      );
    }
    expect(rateLimitRetryAfter).toHaveBeenCalledOnce();
    expect(mockRefresh).toHaveBeenCalledOnce();
  });

  it("stops at the first refusal, saying how many took the tag before it, and sends no more", async () => {
    fetchMock
      .mockResolvedValueOnce(tagged({ team: "core" }))
      .mockResolvedValueOnce(
        await refusedWith(
          400,
          "/errors/telemetry/tags-past-cap",
          "tags past their cap",
          "a project's tag takes up to 500 values a UTC day",
        ),
      );
    expect(await tagTracesAction(PROJECT, [OPEN, OTHER, NEW], { key: "team", value: "core" })).toEqual({
      ok: false,
      error:
        "Tagged 1 of 3 traces before one was refused: tags past their cap: a project's tag takes up to 500 values a UTC day",
    });
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(mockRefresh).toHaveBeenCalledOnce();
  });

  it("refuses runs or a tag it cannot read before asking the lane", async () => {
    const tag = { key: "team", value: "core" };
    for (const [runs, given] of [
      [[], tag],
      [["../views"], tag],
      [Array.from({ length: 501 }, () => OPEN), tag],
      [[OPEN], { key: "team" }],
    ] as const) {
      expect(await tagTracesAction(PROJECT, runs, given)).toEqual({
        ok: false,
        error: "That tag could not be read.",
      });
    }
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
