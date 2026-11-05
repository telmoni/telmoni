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
// on: the dashboards page over a window, opened on a saved dashboard.
const posted = vi.hoisted(() => ({ path: "" }));
vi.mock("next/headers", () => ({
  headers: async () => new Headers({ "x-telmoni-path": posted.path }),
}));

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { getServerSession } from "@/lib/server/session";

import {
  deleteTraceAction,
  deleteTraceViewAction,
  eraseEndUserAction,
  saveTraceViewAction,
  updateTraceViewAction,
} from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);
const PROJECT = "project_1";
const OPEN = "11111111-1111-4111-8111-111111111111";
const OTHER = "22222222-2222-4222-8222-222222222222";
const NEW = "33333333-3333-4333-8333-333333333333";
const PAGE = `/acme/web/dashboards?days=7&view=${OPEN}`;

const input = {
  name: "Team spend",
  query: { charts: [{ measure: "spend", group: "tag", tag: "team" }] },
};
const view = (id: string) =>
  new Response(
    JSON.stringify({
      id,
      ...input,
      created_at: "2026-11-02T00:00:00Z",
      updated_at: "2026-11-02T00:00:00Z",
    }),
    { status: 201 },
  );

beforeEach(() => {
  vi.clearAllMocks();
  posted.path = PAGE;
  vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1", accessToken: "at_1" } as never);
});

describe("saving a new dashboard", () => {
  // The page opens on the dashboard just saved, its menu listing it: the page
  // it was saved from, with the dashboard named, drawn with the answer — not
  // drawn again here and then once more where the browser was sent after.
  it("takes the page to the dashboard saved, everything else the page held kept", async () => {
    fetchMock.mockResolvedValue(view(NEW));
    await expect(saveTraceViewAction(PROJECT, input)).rejects.toThrow(
      `REDIRECT:/acme/web/dashboards?days=7&view=${NEW}`,
    );
    expect(fetchMock).toHaveBeenCalledWith(
      "http://telemetry.test/internal/v1/telemetry/views",
      expect.objectContaining({ method: "POST" }),
    );
    const [, init] = fetchMock.mock.calls[0]!;
    expect(JSON.parse(init!.body as string)).toEqual(input);
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

  // Saved all the same, so the menu has to list it: the page is drawn again.
  it("draws the page again when the lane's answer cannot be read", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 201 }));
    expect(await saveTraceViewAction(PROJECT, input)).toEqual({
      ok: false,
      error: "Unexpected response from telemetry.",
    });
    expect(mockRefresh).toHaveBeenCalledOnce();
  });

  it("refuses what is not a dashboard, or carries a key none holds, before asking the lane", async () => {
    for (const given of [{ name: 3 }, { ...input, kind: "dashboard" }, { ...input, columns: [] }]) {
      expect(await saveTraceViewAction(PROJECT, given)).toEqual({
        ok: false,
        error: "That dashboard could not be read.",
      });
    }
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

describe("saving over a dashboard", () => {
  it("draws the page again where it is, its menu with the change", async () => {
    fetchMock.mockResolvedValue(view(OPEN));
    expect(await updateTraceViewAction(PROJECT, OPEN, input)).toMatchObject({ ok: true });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://telemetry.test/internal/v1/telemetry/views/${OPEN}`,
      expect.objectContaining({ method: "PUT" }),
    );
    expect(mockRefresh).toHaveBeenCalledOnce();
  });
});

describe("deleting a dashboard", () => {
  // The page was opened on it: back to the page, without it.
  it("takes the page opened on it back to the page without it", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    await expect(deleteTraceViewAction(PROJECT, OPEN)).rejects.toThrow(
      "REDIRECT:/acme/web/dashboards?days=7",
    );
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("draws any other page again where it is, its menu without the dashboard", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 204 }));
    expect(await deleteTraceViewAction(PROJECT, OTHER)).toEqual({ ok: true });
    expect(mockRefresh).toHaveBeenCalledOnce();
  });

  it("refuses what is not a dashboard's id before asking the lane", async () => {
    expect(await deleteTraceViewAction(PROJECT, "../views")).toEqual({
      ok: false,
      error: "Invalid dashboard id.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
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
