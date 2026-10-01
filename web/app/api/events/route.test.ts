import { beforeEach, describe, expect, it, vi } from "vitest";

import { logger } from "@/lib/logger";

const mockGetSession = vi.fn();
const mockRateLimit = vi.fn();
const mockIsSessionBlacklisted = vi.fn();
const mockGetServerContext = vi.fn();
const mockSubscribe = vi.fn();
const mockUnsubscribe = vi.fn();
const mockUnsubscribeRevocation = vi.fn();

let messageListener: ((channel: string, message: string) => void) | null = null;
let revocationListener: (() => void) | null = null;

vi.mock("@/lib/auth/session", () => ({
  getSession: () => mockGetSession(),
}));

vi.mock("@/lib/api/rate-limit", () => ({
  rateLimit: (key: string, opts: unknown) => mockRateLimit(key, opts),
  sessionKey: (session: { userId: string }, action: string) =>
    `bfrl:${session.userId}:${action}`,
}));

vi.mock("@/lib/auth/session-blacklist", () => ({
  isSessionBlacklisted: (id: string) => mockIsSessionBlacklisted(id),
  sessionRevokedChannel: (id: string) => `bfev:session:${id}`,
}));

vi.mock("@/lib/server/entities/organization", () => ({
  getServerContext: () => mockGetServerContext(),
}));

vi.mock("@/lib/events/subscriber", () => ({
  subscribe: (
    channels: readonly string[],
    listener: (channel: string, message: string) => void,
  ) => {
    mockSubscribe(channels);
    if (channels.some((c) => c.startsWith("bfev:session:"))) {
      revocationListener = () => listener(channels[0]!, "revoked");
      return mockUnsubscribeRevocation;
    }
    messageListener = listener;
    return mockUnsubscribe;
  },
}));

import { NextRequest } from "next/server";

import { organizationChannel, userChannel } from "@/lib/events/publisher";

import { GET } from "./route";

// Derived rather than written out: the user channel is an HMAC now, so a
// literal here would pin a digest instead of the rule that the stream listens
// on the channel its own address maps to.
const USER_CHANNEL = userChannel("test@example.com");

const LIVE_SESSION = {
  userId: "user_123",
  email: "test@example.com",
  sessionRowId: "sess_live",
  sessionId: null,
};

const INVITE = {
  id: "inv_1",
  scope: "project",
  targetId: "project_1",
  targetName: "My Project",
  role: "admin",
  inviterEmail: "owner@example.com",
  inviterDisplayName: "Owner",
  expiresAt: "2026-09-24T00:00:00Z",
  createdAt: "2026-09-17T00:00:00Z",
};

// The one organization a new account starts with — the person's own, which is
// an entry like any other and shares nothing with their user id.
const OWN = { organizationId: "org_own", ownerEmail: "test@example.com", role: "owner" };

describe("GET /api/events (SSE)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockIsSessionBlacklisted.mockResolvedValue(false);
    mockRateLimit.mockResolvedValue(null);
    mockGetServerContext.mockResolvedValue({ organizations: [OWN] });
    messageListener = null;
    revocationListener = null;
  });

  it("returns 403 if sec-fetch-site is cross-site", async () => {
    const req = new NextRequest("http://localhost:3000/api/events", {
      headers: { "sec-fetch-site": "cross-site" },
    });

    const res = await GET(req);
    expect(res.status).toBe(403);
    const body = await res.json();
    expect(body.error).toBe("forbidden");
  });

  it("returns 401 if unauthenticated", async () => {
    mockGetSession.mockResolvedValue(null);
    const req = new NextRequest("http://localhost:3000/api/events");

    const res = await GET(req);
    expect(res.status).toBe(401);
  });

  it("returns 401 if session is blacklisted", async () => {
    mockGetSession.mockResolvedValue({ ...LIVE_SESSION, sessionRowId: "sess_revoked" });
    mockIsSessionBlacklisted.mockResolvedValue(true);
    const req = new NextRequest("http://localhost:3000/api/events");

    const res = await GET(req);
    expect(res.status).toBe(401);
  });

  it("returns text/event-stream headers and subscribes to the user's own channels", async () => {
    mockGetSession.mockResolvedValue(LIVE_SESSION);
    const req = new NextRequest("http://localhost:3000/api/events");

    const res = await GET(req);
    expect(res.status).toBe(200);
    expect(res.headers.get("content-type")).toContain("text/event-stream");
    expect(res.headers.get("cache-control")).toBe("no-cache, no-transform");

    await res.body?.getReader().read();
    expect(mockSubscribe).toHaveBeenCalledWith([
      USER_CHANNEL,
      organizationChannel("org_own"),
    ]);
  });

  it("also listens on every organization the user is a member of", async () => {
    mockGetSession.mockResolvedValue(LIVE_SESSION);
    mockGetServerContext.mockResolvedValue({
      organizations: [
        OWN,
        { organizationId: "org_x", ownerEmail: "x@example.com", role: "member" },
        { organizationId: "org_z", ownerEmail: "z@example.com", role: "admin" },
      ],
    });
    const req = new NextRequest("http://localhost:3000/api/events");

    const res = await GET(req);
    await res.body?.getReader().read();
    expect(mockSubscribe).toHaveBeenCalledWith([
      USER_CHANNEL,
      organizationChannel("org_own"),
      organizationChannel("org_x"),
      organizationChannel("org_z"),
    ]);
  });

  // ⚠ The stream subscribes to the organization channel the person
  // actually owns, rather than user id.
  it("never listens on a channel named for the user id", async () => {
    mockGetSession.mockResolvedValue(LIVE_SESSION);
    const res = await GET(new NextRequest("http://localhost:3000/api/events"));
    await res.body?.getReader().read();

    const channels = mockSubscribe.mock.calls.flatMap(([c]) => c as string[]);
    expect(channels).not.toContain(organizationChannel("user_123"));
  });

  it("answers 503 when auth cannot list the caller's organizations, rather than a stream that will never hear them", async () => {
    mockGetSession.mockResolvedValue(LIVE_SESSION);
    mockGetServerContext.mockResolvedValue(null);
    const req = new NextRequest("http://localhost:3000/api/events");

    const res = await GET(req);
    expect(res.status).toBe(503);
    expect(res.headers.get("retry-after")).toBe("30");
    expect(mockSubscribe).not.toHaveBeenCalled();
  });

  it("emits events to the SSE stream when received from Redis", async () => {
    mockGetSession.mockResolvedValue(LIVE_SESSION);
    const req = new NextRequest("http://localhost:3000/api/events");

    const res = await GET(req);
    const reader = res.body?.getReader();
    expect(reader).toBeDefined();

    const initial = await reader?.read();
    const initialText = new TextDecoder().decode(initial?.value);
    expect(initialText).toContain(": connected\n\n");

    expect(messageListener).toBeDefined();
    messageListener?.(
      USER_CHANNEL,
      JSON.stringify({ type: "invite:created", data: INVITE }),
    );

    const chunk = await reader?.read();
    const chunkText = new TextDecoder().decode(chunk?.value);
    expect(chunkText).toContain("event: invite:created\n");
    expect(chunkText).toContain('"id":"inv_1"');

    await reader?.cancel();
  });

  // An offer made, withdrawn or answered moves roles the console only learns
  // from `/me`, so the event carries the organization and nothing else.
  it("relays an ownership change on the organization's channel as it came", async () => {
    mockGetSession.mockResolvedValue(LIVE_SESSION);
    const res = await GET(new NextRequest("http://localhost:3000/api/events"));
    const reader = res.body!.getReader();
    await reader.read();

    messageListener?.(
      organizationChannel("org_own"),
      JSON.stringify({ type: "ownership:changed", data: { organizationId: "org_own" } }),
    );

    const chunk = await reader.read();
    expect(new TextDecoder().decode(chunk.value)).toBe(
      'event: ownership:changed\ndata: {"organizationId":"org_own"}\n\n',
    );
    await reader.cancel();
  });

  it("drops a message that is not one of our events, and says so", async () => {
    mockGetSession.mockResolvedValue(LIVE_SESSION);
    const warn = vi.spyOn(logger, "warn").mockImplementation(() => {});
    const res = await GET(new NextRequest("http://localhost:3000/api/events"));
    const reader = res.body?.getReader();
    await reader?.read();

    messageListener?.(
      USER_CHANNEL,
      JSON.stringify({
        type: "invite:created\ndata: {}\n\nevent: close",
        data: INVITE,
      }),
    );
    messageListener?.(USER_CHANNEL, JSON.stringify({ hello: 1 }));
    messageListener?.(
      USER_CHANNEL,
      JSON.stringify({ type: "invite:created", data: { id: "inv_1" } }),
    );

    expect(warn).toHaveBeenCalled();

    messageListener?.(
      USER_CHANNEL,
      JSON.stringify({ type: "invite:created", data: INVITE }),
    );
    const chunk = await reader?.read();
    const text = new TextDecoder().decode(chunk?.value);
    expect(text).toBe(
      `event: invite:created\ndata: ${JSON.stringify(INVITE)}\n\n`,
    );

    warn.mockRestore();
    await reader?.cancel();
  });

  it("stops listening when the client goes away", async () => {
    mockGetSession.mockResolvedValue(LIVE_SESSION);
    const req = new NextRequest("http://localhost:3000/api/events");

    const res = await GET(req);
    const reader = res.body?.getReader();
    await reader?.read();
    expect(mockUnsubscribe).not.toHaveBeenCalled();

    await reader?.cancel();
    expect(mockUnsubscribe).toHaveBeenCalledTimes(1);
    expect(mockUnsubscribeRevocation).toHaveBeenCalledTimes(1);

    expect(() =>
      messageListener?.(USER_CHANNEL, JSON.stringify({ type: "invite:revoked", data: {} })),
    ).not.toThrow();
  });

  it("closes the stream the instant its session is revoked, without waiting for the poll", async () => {
    mockGetSession.mockResolvedValue(LIVE_SESSION);
    const res = await GET(new NextRequest("http://localhost:3000/api/events"));
    const reader = res.body!.getReader();
    await reader.read();
    expect(mockSubscribe).toHaveBeenCalledWith(["bfev:session:sess_live"]);

    revocationListener?.();

    const chunk = await reader.read();
    expect(new TextDecoder().decode(chunk.value)).toBe(
      'event: close\ndata: {"reason":"session_revoked"}\n\n',
    );
    const end = await reader.read();
    expect(end.done).toBe(true);
    expect(mockUnsubscribe).toHaveBeenCalledTimes(1);
    expect(mockUnsubscribeRevocation).toHaveBeenCalledTimes(1);
  });

  it("listens for no revocation when the session names neither a row nor a sid", async () => {
    mockGetSession.mockResolvedValue({ ...LIVE_SESSION, sessionRowId: null });
    const res = await GET(new NextRequest("http://localhost:3000/api/events"));
    await res.body?.getReader().read();

    const channels = mockSubscribe.mock.calls.flatMap(([c]) => c as string[]);
    expect(channels.some((c) => c.startsWith("bfev:session:"))).toBe(false);
  });

  // The channel and the blacklist key must agree, or an account deletion
  // that ends a row-less session under its session id would close this
  // stream only on the next keepalive poll.
  it("listens on the session id when the session has no row", async () => {
    mockGetSession.mockResolvedValue({ ...LIVE_SESSION, sessionRowId: null, sessionId: "sid_1" });
    const res = await GET(new NextRequest("http://localhost:3000/api/events"));
    await res.body?.getReader().read();

    const channels = mockSubscribe.mock.calls.flatMap(([c]) => c as string[]);
    expect(channels).toContain("bfev:session:sid_1");
  });

  it("ceilings stream opens per session, before spending a /me round trip", async () => {
    mockGetSession.mockResolvedValue(LIVE_SESSION);
    const { NextResponse } = await import("next/server");
    mockRateLimit.mockResolvedValueOnce(
      NextResponse.json({ error: "rate limit exceeded" }, { status: 429, headers: { "retry-after": "17" } }),
    );

    const res = await GET(new NextRequest("http://localhost:3000/api/events"));
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBe("17");
    expect(mockRateLimit).toHaveBeenCalledWith("bfrl:user_123:events:open", {
      limit: 60,
      windowMs: 60_000,
    });
    expect(mockGetServerContext).not.toHaveBeenCalled();
    expect(mockSubscribe).not.toHaveBeenCalled();
  });
});
