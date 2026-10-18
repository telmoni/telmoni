import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
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
  activeProjectForMutation: vi.fn(async (projectId: string) => ({
    organizationId: "org_1",
    projectId,
    error: null,
  })),
}));
vi.mock("@/lib/server/entities/identity-context", () => ({
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

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { getServerSession } from "@/lib/server/session";

import {
  deleteTraceViewAction,
  saveTraceViewAction,
  setSessionLayoutAction,
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
  kind: "list",
  query: { agent: ["support"], days: 7 },
  columns: [],
};
const view = (id: string) =>
  new Response(
    JSON.stringify({
      id,
      kind: "list",
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
      "http://telemetry.test/internal/telemetry/views",
      expect.objectContaining({ method: "POST" }),
    );
    expect(mockRefresh).not.toHaveBeenCalled();
  });

  it("stays, saying why, when the lane refuses it", async () => {
    fetchMock.mockResolvedValue(new Response(null, { status: 409 }));
    expect(await saveTraceViewAction(PROJECT, input)).toEqual({
      status: "error",
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
    ).toEqual({ status: "error", error: "That view could not be read." });
    expect(
      await saveTraceViewAction(PROJECT, { ...layoutInput, kind: "list" }),
    ).toEqual({ status: "error", error: "That view could not be read." });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  // Saved all the same, so the menu has to list it: the page is drawn again.
  it("draws the list again when the lane's answer cannot be read", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 201 }));
    expect(await saveTraceViewAction(PROJECT, input)).toEqual({
      status: "error",
      error: "Unexpected response from telemetry.",
    });
    expect(mockRefresh).toHaveBeenCalledOnce();
  });

  it("refuses what is not a view before asking the lane", async () => {
    expect(await saveTraceViewAction(PROJECT, { name: 3 })).toEqual({
      status: "error",
      error: "That view could not be read.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("tells a session that ran out to sign in again, asking the lane nothing", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null);
    expect(await saveTraceViewAction(PROJECT, input)).toEqual({
      status: "error",
      error: "Your session expired — sign in again.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("saving over a view", () => {
  it("draws the list again where it is, its menu with the change", async () => {
    fetchMock.mockResolvedValue(view(OPEN));
    expect(await updateTraceViewAction(PROJECT, OPEN, input)).toMatchObject({ status: "ok" });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://telemetry.test/internal/telemetry/views/${OPEN}`,
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
    expect(await deleteTraceViewAction(PROJECT, OTHER)).toEqual({ status: "ok" });
    expect(mockRefresh).toHaveBeenCalledOnce();
  });

  it("refuses what is not a view's id before asking the lane", async () => {
    expect(await deleteTraceViewAction(PROJECT, "../views")).toEqual({
      status: "error",
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
      status: "ok",
      setting: { view_id: OPEN },
    });
    expect(fetchMock).toHaveBeenCalledWith(
      "http://telemetry.test/internal/telemetry/session-layout",
      expect.objectContaining({ method: "PUT", body: JSON.stringify({ view_id: OPEN }) }),
    );
    expect(mockRefresh).toHaveBeenCalledOnce();

    fetchMock.mockResolvedValue(setting({ view_id: null }));
    expect(await setSessionLayoutAction(PROJECT, { view_id: null })).toEqual({
      status: "ok",
      setting: { view_id: null },
    });
  });

  it("refuses what is no layout before asking the lane, and says why the lane refused", async () => {
    for (const input of [{}, { layout: "complex" }, { view_id: "mine" }, { view_id: OPEN, layout: "simple" }]) {
      expect(await setSessionLayoutAction(PROJECT, input)).toEqual({
        status: "error",
        error: "That layout could not be read.",
      });
    }
    expect(fetchMock).not.toHaveBeenCalled();
    fetchMock.mockResolvedValue(new Response(null, { status: 404 }));
    expect(await setSessionLayoutAction(PROJECT, { view_id: OTHER })).toEqual({
      status: "error",
      error: "a view by that name exists",
    });
    expect(mockRefresh).not.toHaveBeenCalled();
  });
});
