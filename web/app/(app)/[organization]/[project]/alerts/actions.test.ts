import { beforeEach, describe, expect, it, vi } from "vitest";

import { errorOf } from "@/lib/action-result";

vi.mock("next/cache", () => ({
  refresh: vi.fn(),
}));
vi.mock("@/lib/server/session", () => ({
  getServerSession: vi.fn(),
}));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimitRetryAfter: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "this project has an alert by that name already" })),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://server.test",
    SERVICE_SECRET: "secret",
  },
}));
// The real `sessionHeaders`: what reaches the server is the assertion.
vi.mock("@/lib/server/entities/identity-context", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/server/entities/identity-context")>()),
  identityContext: vi.fn(),
}));
vi.mock("@/lib/server/entities/organization", () => ({
  getServerContext: vi.fn(async () => null),
}));

import { refresh } from "next/cache";

import { rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { identityContext } from "@/lib/server/entities/identity-context";
import { getServerSession } from "@/lib/server/session";

import {
  deleteAlertAction,
  endMuteAction,
  hearAgainAction,
  incidentAction,
  muteAction,
  pauseAlertAction,
  saveAlertAction,
} from "./actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);

const PROJECT = "project_7bQx2mNv9BcK4dLp";
const RULE = "7a1c0e52-0000-4000-8000-000000000002";
const INCIDENT = "5d4f5c68-0000-4000-8000-000000000001";
const CONNECTION = "3c4e2a74-0000-4000-8000-000000000004";
const MUTE = "1e2f3a4b-0000-4000-8000-000000000005";
const BASE = "http://server.test/internal/v1/telemetry";

const INPUT = {
  name: "Slow calls",
  kind: "model_call",
  measure: "duration",
  aggregation: "p95",
  filters: { model: ["claude-sonnet-5-5"] },
  window: "hour",
  operator: "gt",
  alert_threshold: 5000,
  warning_threshold: 3000,
  missing_data: "zero",
  renotify_minutes: null,
  connectors: [{ connection_id: CONNECTION, level: "alert", hours: null }],
};

/** The one request the action made: its URL, method and body. */
function sent() {
  expect(fetchMock).toHaveBeenCalledOnce();
  const [url, init] = fetchMock.mock.calls[0];
  return { url, method: init?.method, body: init?.body ? JSON.parse(String(init.body)) : undefined, init };
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_1",
    email: "user@example.test",
    accessToken: "at_1",
  } as never);
  vi.mocked(identityContext).mockResolvedValue({
    userId: "user_1",
    organizationId: "org_acme",
    role: "member",
    accessToken: "at_1",
  });
  fetchMock.mockResolvedValue(new Response("{}", { status: 200 }));
});

describe("saveAlertAction", () => {
  it("makes a new alert, as the person, on the project, and draws the page again", async () => {
    expect(await saveAlertAction(PROJECT, null, INPUT)).toEqual({ ok: true });
    const { url, method, body, init } = sent();
    expect([url, method]).toEqual([`${BASE}/rules`, "POST"]);
    expect(body).toEqual(INPUT);
    expect(init?.headers).toMatchObject({
      authorization: "Bearer at_1",
      "x-organization-id": "org_acme",
      "x-project-id": PROJECT,
      "content-type": "application/json",
    });
    expect(refresh).toHaveBeenCalledOnce();
  });

  it("saves one over whole, by its id", async () => {
    expect(await saveAlertAction(PROJECT, RULE, INPUT)).toEqual({ ok: true });
    expect(sent()).toMatchObject({ url: `${BASE}/rules/${RULE}`, method: "PUT", body: INPUT });
  });

  // A Server Action takes whatever is posted to it.
  it.each<[string | null, unknown]>([
    ["../mutes", INPUT],
    [`${RULE}/pause`, INPUT],
    [null, { ...INPUT, owner: "user_2" }],
    [null, { ...INPUT, filters: { user: ["u1"] } }],
    [null, { ...INPUT, kind: "span" }],
    [null, { ...INPUT, alert_threshold: "5000" }],
    [null, { ...INPUT, connectors: [{ connection_id: "../../rules", level: "alert", hours: null }] }],
    [null, null],
  ])("refuses the alert %j, %j, asking the server nothing", async (ruleId, input) => {
    const res = await saveAlertAction(PROJECT, ruleId, input);
    expect(errorOf(res)).toMatch(/Couldn't tell which alert|could not be read/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("says what the lane refused, or that it could not be reached, and draws nothing again", async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 409 }));
    expect(await saveAlertAction(PROJECT, null, INPUT)).toEqual({
      ok: false,
      error: "this project has an alert by that name already",
    });
    fetchMock.mockResolvedValueOnce(null);
    expect(errorOf(await saveAlertAction(PROJECT, null, INPUT))).toMatch(/Alerts are unavailable/);
    expect(refresh).not.toHaveBeenCalled();
  });

  // Another member deleted it: the page is drawn again without it.
  it("draws the page again where what it acted on is gone, and answers that it is", async () => {
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 404 }));
    expect(await saveAlertAction(PROJECT, RULE, INPUT)).toEqual({ ok: false, error: expect.any(String), gone: true });
    expect(refresh).toHaveBeenCalledOnce();
  });

  it("asks nothing for an expired session, past the minute's room, or for another header", async () => {
    vi.mocked(getServerSession).mockResolvedValueOnce(null);
    expect(errorOf(await saveAlertAction(PROJECT, null, INPUT))).toMatch(/session expired/);
    vi.mocked(rateLimitRetryAfter).mockResolvedValueOnce(30);
    expect(errorOf(await saveAlertAction(PROJECT, null, INPUT))).toMatch(/Too many requests/);
    expect(errorOf(await saveAlertAction("p\r\nx-project-id: q", null, INPUT))).toMatch(
      /Couldn't tell which project/,
    );
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("the alert's own changes", () => {
  it("pauses and resumes an alert", async () => {
    await pauseAlertAction(PROJECT, RULE, true);
    expect(sent()).toMatchObject({ url: `${BASE}/rules/${RULE}/pause`, method: "POST", body: undefined });
    fetchMock.mockClear();
    await pauseAlertAction(PROJECT, RULE, false);
    expect(sent()).toMatchObject({ url: `${BASE}/rules/${RULE}/resume`, method: "POST" });
  });

  it("deletes an alert, and draws one another member deleted first as gone", async () => {
    expect(await deleteAlertAction(PROJECT, RULE)).toEqual({ ok: true });
    expect(sent()).toMatchObject({ url: `${BASE}/rules/${RULE}`, method: "DELETE" });
    fetchMock.mockResolvedValueOnce(new Response(null, { status: 404 }));
    expect(await deleteAlertAction(PROJECT, RULE)).toEqual({ ok: true });
    expect(refresh).toHaveBeenCalledTimes(2);
  });

  it("hears a muted connector again", async () => {
    await hearAgainAction(PROJECT, RULE, CONNECTION);
    expect(sent()).toMatchObject({
      url: `${BASE}/rules/${RULE}/connectors/${CONNECTION}/unmute`,
      method: "POST",
    });
  });

  it("acknowledges and resolves an incident", async () => {
    await incidentAction(PROJECT, INCIDENT, "acknowledge");
    expect(sent()).toMatchObject({ url: `${BASE}/incidents/${INCIDENT}/acknowledge`, method: "POST" });
    fetchMock.mockClear();
    await incidentAction(PROJECT, INCIDENT, "resolve");
    expect(sent()).toMatchObject({ url: `${BASE}/incidents/${INCIDENT}/resolve`, method: "POST" });
  });

  it("refuses ids and verbs that could reach past their segment", async () => {
    for (const res of [
      await pauseAlertAction(PROJECT, "../incidents", true),
      await pauseAlertAction(PROJECT, RULE, "yes" as unknown as boolean),
      await deleteAlertAction(PROJECT, `${RULE}/pause`),
      await hearAgainAction(PROJECT, RULE, "../../rules"),
      await incidentAction(PROJECT, INCIDENT, "../../rules" as "resolve"),
      await incidentAction(PROJECT, "x", "resolve"),
      await endMuteAction(PROJECT, `${MUTE}/end`),
    ]) {
      expect(errorOf(res)).toMatch(/Couldn't tell which/);
    }
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("maintenance windows", () => {
  // Both ends on one clock: a thirty-day window is never read a moment longer.
  it("mutes from now for its length, both ends spelled on this clock", async () => {
    vi.useFakeTimers({ now: Date.parse("2026-10-26T12:00:00Z") });
    try {
      await muteAction(PROJECT, { rule_id: RULE, starts_at: null, minutes: 43_200 });
    } finally {
      vi.useRealTimers();
    }
    expect(sent()).toMatchObject({
      url: `${BASE}/mutes`,
      method: "POST",
      body: {
        rule_id: RULE,
        starts_at: "2026-10-26T12:00:00.000Z",
        ends_at: "2026-11-25T12:00:00.000Z",
      },
    });
  });

  it("mutes every alert from a set time", async () => {
    await muteAction(PROJECT, { rule_id: null, starts_at: "2026-10-27T08:00:00.000Z", minutes: 60 });
    expect(sent().body).toEqual({
      rule_id: null,
      starts_at: "2026-10-27T08:00:00.000Z",
      ends_at: "2026-10-27T09:00:00.000Z",
    });
  });

  it.each<unknown>([
    { rule_id: RULE, starts_at: null, minutes: 43_201 },
    { rule_id: RULE, starts_at: null, minutes: 0 },
    { rule_id: RULE, starts_at: "tomorrow", minutes: 60 },
    { rule_id: "../rules", starts_at: null, minutes: 60 },
    { rule_id: RULE, starts_at: null, minutes: 60, ends_at: "2027-01-01T00:00:00Z" },
  ])("refuses the window %j, asking the server nothing", async (input) => {
    expect(errorOf(await muteAction(PROJECT, input))).toMatch(/could not be read/);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("ends a window now", async () => {
    await endMuteAction(PROJECT, MUTE);
    expect(sent()).toMatchObject({ url: `${BASE}/mutes/${MUTE}/end`, method: "POST" });
  });
});
