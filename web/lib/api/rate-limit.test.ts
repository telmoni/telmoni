import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const evalMock = vi.fn();
const defineMock = vi.fn();
const redisMock = {
  slidingWindow: evalMock,
  defineCommand: defineMock,
  on: vi.fn(),
  status: "ready",
};

vi.mock("@/lib/redis", () => ({
  getRedis: () => redisMock,
}));

import { clientKey, rateLimit, sessionKey } from "./rate-limit";

describe("sessionKey", () => {
  it("keys on the user — every session individually throttled", () => {
    expect(sessionKey({ userId: "u1" }, "specs:create"))
      .toBe("bfrl:u1:specs:create");
  });
});

describe("clientKey", () => {
  it("prioritizes cf-connecting-ip when present from edge ingress", () => {
    const req = new Request("http://example.com", {
      headers: {
        "cf-connecting-ip": "203.0.113.195",
        "x-forwarded-for": "1.2.3.4, 5.6.7.8, 9.9.9.9",
      },
    });
    expect(clientKey(req as never, "unsubscribe"))
      .toBe("bfrl:unsubscribe:203.0.113.195");
  });

  it("keys on the trusted second-from-last hop, not the spoofable first", () => {
    const req = new Request("http://example.com", {
      headers: { "x-forwarded-for": "1.2.3.4, 5.6.7.8, 9.9.9.9" },
    });
    expect(clientKey(req as never, "unsubscribe"))
      .toBe("bfrl:unsubscribe:5.6.7.8");
  });

  it("takes the sole hop when only one element is present", () => {
    const req = new Request("http://example.com", {
      headers: { "x-forwarded-for": "1.2.3.4" },
    });
    expect(clientKey(req as never, "unsubscribe"))
      .toBe("bfrl:unsubscribe:1.2.3.4");
  });

  it("falls back to x-real-ip then to 'unknown'", () => {
    const noXff = new Request("http://example.com", {
      headers: { "x-real-ip": "9.9.9.9" },
    });
    expect(clientKey(noXff as never, "unsubscribe"))
      .toBe("bfrl:unsubscribe:9.9.9.9");

    const naked = new Request("http://example.com");
    expect(clientKey(naked as never, "unsubscribe"))
      .toBe("bfrl:unsubscribe:unknown");
  });
});

describe("rateLimit (Redis backend)", () => {
  beforeEach(() => {
    evalMock.mockReset();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("returns null when the Lua script admits the request", async () => {
    evalMock.mockResolvedValueOnce([1, 0]);
    const res = await rateLimit("bfrl:o1:tokens:mint", { limit: 5, windowMs: 60_000 });
    expect(res).toBeNull();
    expect(evalMock).toHaveBeenCalledOnce();
    const [key, windowMs, limit, member] = evalMock.mock.calls[0]!;
    expect(key).toBe("bfrl:o1:tokens:mint");
    expect(windowMs).toBe("60000");
    expect(limit).toBe("5");
    expect(member).toEqual(expect.any(String));
  });

  // ⚠ **No clock crosses the wire, and that is the assertion.** One bucket is
  // shared by every replica, so a caller-supplied `now` meant each of them
  // scored the same sorted set by its own clock.
  it("sends the server no opinion about what time it is", () => {
    evalMock.mockResolvedValueOnce([1, 0]);
    return rateLimit("bfrl:o1:tokens:mint", { limit: 5, windowMs: 60_000 }).then(() => {
      const args = evalMock.mock.calls[0]!;
      expect(args).toHaveLength(4);
      for (const arg of args) {
        expect(String(arg)).not.toMatch(/^1[0-9]{12}$/);
      }
    });
  });

  it("registers the script as a command once, so every call after the first is an EVALSHA", async () => {
    evalMock.mockResolvedValue([1, 0]);
    await rateLimit("bfrl:o1:a", { limit: 5, windowMs: 60_000 });
    await rateLimit("bfrl:o1:b", { limit: 5, windowMs: 60_000 });
    expect(defineMock).toHaveBeenCalledTimes(1);
    const [name, definition] = defineMock.mock.calls[0]!;
    expect(name).toBe("slidingWindow");
    expect(definition.numberOfKeys).toBe(1);
    expect(definition.lua).toContain("ZREMRANGEBYSCORE");
    expect(definition.lua).toContain("PEXPIRE");
    expect(definition.lua, "the window must be measured server-side").toContain(
      'redis.call("TIME")',
    );
  });

  it("returns 429 with Retry-After when the Lua script rejects", async () => {
    evalMock.mockResolvedValueOnce([0, 4_200]);
    const res = await rateLimit("bfrl:o1:tokens:mint", { limit: 5, windowMs: 60_000 });
    expect(res).not.toBeNull();
    expect(res!.status).toBe(429);
    expect(res!.headers.get("retry-after")).toBe("5");
  });

  it("falls back to in-memory when Redis errors", async () => {
    evalMock.mockRejectedValueOnce(new Error("ECONNREFUSED"));
    const res = await rateLimit("bfrl:fallback:test", { limit: 1, windowMs: 60_000 });
    expect(res).toBeNull();
    evalMock.mockRejectedValueOnce(new Error("ECONNREFUSED"));
    const res2 = await rateLimit("bfrl:fallback:test", { limit: 1, windowMs: 60_000 });
    expect(res2).not.toBeNull();
    expect(res2!.status).toBe(429);
  });

  it("in-memory fallback limits correctly", async () => {
    evalMock.mockRejectedValue(new Error("ECONNREFUSED"));
    const opts = { limit: 2, windowMs: 60_000 };
    const r1 = await rateLimit("bfrl:mem:test", opts);
    expect(r1).toBeNull();
    const r2 = await rateLimit("bfrl:mem:test", opts);
    expect(r2).toBeNull();
    const r3 = await rateLimit("bfrl:mem:test", opts);
    expect(r3).not.toBeNull();
    expect(r3!.status).toBe(429);
  });
});
