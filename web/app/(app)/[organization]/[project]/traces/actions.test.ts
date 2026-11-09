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
// The page the action was posted from, whose path and query the proxy hands on.
const posted = vi.hoisted(() => ({ path: "" }));
vi.mock("next/headers", () => ({
  headers: async () => new Headers({ "x-telmoni-path": posted.path }),
}));

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { getServerSession } from "@/lib/server/session";

import { deleteTraceAction, eraseEndUserAction } from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);
const PROJECT = "project_1";
const OPEN = "11111111-1111-4111-8111-111111111111";

beforeEach(() => {
  vi.clearAllMocks();
  posted.path = `/acme/web/traces/${OPEN}`;
  vi.mocked(getServerSession).mockResolvedValue({ userId: "user_1", accessToken: "at_1" } as never);
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

  it("tells a session that ran out to sign in again, asking the lane nothing", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null);
    expect(await deleteTraceAction(PROJECT, OPEN)).toEqual({
      ok: false,
      error: "Your session expired — sign in again.",
    });
    expect(fetchMock).not.toHaveBeenCalled();
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
