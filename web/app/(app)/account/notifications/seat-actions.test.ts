import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("next/cache", () => ({ refresh: vi.fn() }));
vi.mock("@/lib/server/session", () => ({ getServerSession: vi.fn() }));
vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: vi.fn(async () => null),
  sessionKey: vi.fn(() => "k"),
}));
vi.mock("@/lib/server/entities/identity-context", () => ({
  personHeaders: vi.fn(() => ({ authorization: "Bearer at_1" })),
}));
vi.mock("@/lib/api/fetch", () => ({
  tryFetchWithTimeout: vi.fn(),
  extractProblem: vi.fn(async () => ({ message: "not found" })),
}));
vi.mock("@/lib/env", () => ({ env: { SERVER_URL: "http://auth.test" } }));
const publishEvent = vi.hoisted(() => vi.fn());
vi.mock("@/lib/events/publisher", () => ({
  publishEvent,
  userChannel: (email: string) => `bfev:user:${email.toLowerCase()}`,
}));

import { refresh } from "next/cache";

import { tryFetchWithTimeout } from "@/lib/api/fetch";
import { getServerSession } from "@/lib/server/session";

import { markSeatSeenAction } from "./seat-actions";

const fetchMock = vi.mocked(tryFetchWithTimeout);
const PROJECT = "project_7bQx2mNv9BcK4dLp";

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(getServerSession).mockResolvedValue({
    userId: "user_dana",
    email: "Dana@example.test",
    accessToken: "at_1",
  } as never);
});

// The person's own lane marks the seat seen; their other consoles are told on
// their own channel, by the organization auth answered for the seat.
describe("markSeatSeenAction", () => {
  it("marks the seat seen under the person's bearer and tells their other consoles", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ organizationId: "org_acme" }), { status: 200 }),
    );
    expect(await markSeatSeenAction(PROJECT)).toEqual({ error: null });
    expect(fetchMock).toHaveBeenCalledWith(
      `http://auth.test/internal/v1/me/seats/${PROJECT}/seen`,
      expect.objectContaining({ method: "POST", headers: { authorization: "Bearer at_1" } }),
    );
    expect(publishEvent).toHaveBeenCalledExactlyOnceWith("bfev:user:dana@example.test", {
      type: "membership:changed",
      data: { organizationId: "org_acme", projectId: PROJECT },
    });
    expect(vi.mocked(refresh)).toHaveBeenCalled();
  });

  it("tells nobody when no seat was waiting", async () => {
    fetchMock.mockResolvedValue(
      new Response(JSON.stringify({ organizationId: null }), { status: 200 }),
    );
    expect(await markSeatSeenAction(PROJECT)).toEqual({ error: null });
    expect(publishEvent).not.toHaveBeenCalled();
  });

  it("refuses anything but a project id before any request", async () => {
    const r = await markSeatSeenAction("../organization");
    expect(r.error).toMatch(/resolve that project/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("asks for a sign-in when the session is gone", async () => {
    vi.mocked(getServerSession).mockResolvedValue(null as never);
    expect((await markSeatSeenAction(PROJECT)).error).toMatch(/sign in again/i);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("hands back auth's refusal, and tells nobody", async () => {
    fetchMock.mockResolvedValue(new Response("{}", { status: 404 }));
    expect(await markSeatSeenAction(PROJECT)).toEqual({ error: "not found" });
    expect(publishEvent).not.toHaveBeenCalled();
  });
});
