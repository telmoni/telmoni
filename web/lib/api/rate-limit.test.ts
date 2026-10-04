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

import { logger } from "@/lib/logger";

import { clientKey, rateLimit, sessionKey } from "./rate-limit";

describe("sessionKey", () => {
  it("keys on the user — every session individually throttled", () => {
    expect(sessionKey({ userId: "u1" }, "specs:create"))
      .toBe("bfrl:u1:specs:create");
  });
});

describe("clientKey", () => {
  afterEach(() => {
    delete process.env.TRUSTED_PROXY_HOPS;
  });

  // Unset is Google's load balancer: the client's address and then its own.
  it("keys on the trusted second-from-last hop, not the spoofable first", () => {
    const req = new Request("http://example.com", {
      headers: { "x-forwarded-for": "1.2.3.4, 5.6.7.8, 9.9.9.9" },
    });
    expect(clientKey(req as never, "unsubscribe"))
      .toBe("bfrl:unsubscribe:5.6.7.8");
  });

  // ⚠ Nothing in front of the console sets these, so a caller can: read ahead
  // of the load balancer's hop, a fresh value per request is a fresh bucket
  // per request, and every per-address ceiling admits everything.
  it.each(["cf-connecting-ip", "true-client-ip", "x-client-ip"])(
    "takes no address from %s, which a caller can write",
    (header) => {
      const req = new Request("http://example.com", {
        headers: {
          [header]: "198.51.100.7",
          "x-forwarded-for": "1.2.3.4, 5.6.7.8, 9.9.9.9",
        },
      });
      expect(clientKey(req as never, "unsubscribe")).toBe("bfrl:unsubscribe:5.6.7.8");
    },
  );

  it("takes the sole hop when only one element is present", () => {
    const req = new Request("http://example.com", {
      headers: { "x-forwarded-for": "1.2.3.4" },
    });
    expect(clientKey(req as never, "unsubscribe"))
      .toBe("bfrl:unsubscribe:1.2.3.4");
  });

  // One reverse proxy appends the client's address and nothing more, so the
  // last entry is the client, and anything a caller wrote sits in front of it.
  it("reads one hop back behind a single proxy", () => {
    process.env.TRUSTED_PROXY_HOPS = "1";
    const forged = new Request("http://example.com", {
      headers: { "x-forwarded-for": "198.51.100.7, 5.6.7.8" },
    });
    expect(clientKey(forged as never, "unsubscribe")).toBe("bfrl:unsubscribe:5.6.7.8");
  });

  // Nothing in front: the last entry is what Next recorded from the socket
  // when the caller sent no header, and the caller's word when it did.
  it("takes the last entry when no proxy is trusted", () => {
    process.env.TRUSTED_PROXY_HOPS = "0";
    const req = new Request("http://example.com", {
      headers: { "x-forwarded-for": "1.2.3.4, 5.6.7.8" },
    });
    expect(clientKey(req as never, "unsubscribe")).toBe("bfrl:unsubscribe:5.6.7.8");
  });

  // `x-real-ip` is a request header like any other here: nothing in front of
  // the console sets it, so a caller could.
  it("takes no address from x-real-ip, and keys an unaddressed request as unknown", () => {
    const noXff = new Request("http://example.com", {
      headers: { "x-real-ip": "9.9.9.9" },
    });
    expect(clientKey(noXff as never, "unsubscribe"))
      .toBe("bfrl:unsubscribe:unknown");

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
});

describe("a limit of zero denies on BOTH backends", () => {
  beforeEach(() => {
    evalMock.mockReset();
  });

  it("returns a usable Retry-After from the in-memory backstop", async () => {
    evalMock.mockRejectedValueOnce(new Error("ECONNREFUSED"));
    const res = await rateLimit("bfrl:zero:memory", { limit: 0, windowMs: 60_000 });
    expect(res).not.toBeNull();
    expect(res!.status).toBe(429);
    const retryAfter = res!.headers.get("retry-after")!;
    expect(retryAfter).not.toBe("NaN");
    expect(Number.isFinite(Number(retryAfter))).toBe(true);
    expect(Number(retryAfter)).toBeGreaterThan(0);
  });
});

describe("a client that exists but is not ready", () => {
  beforeEach(() => {
    evalMock.mockReset();
  });
  afterEach(() => {
    redisMock.status = "ready";
  });

  it("uses the per-instance limiter and says so once a minute, not once a request", async () => {
    redisMock.status = "reconnecting";
    const warn = vi.spyOn(logger, "warn").mockImplementation(() => logger);
    try {
      const opts = { limit: 1, windowMs: 60_000 };
      expect(await rateLimit("bfrl:notready:a", opts)).toBeNull();
      const second = await rateLimit("bfrl:notready:a", opts);
      expect(second?.status).toBe(429);
      expect(evalMock).not.toHaveBeenCalled();
      expect(warn).toHaveBeenCalledTimes(1);
    } finally {
      warn.mockRestore();
    }
  });
});
