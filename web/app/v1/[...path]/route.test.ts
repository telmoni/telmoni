// @vitest-environment node
import { beforeEach, describe, expect, it, vi } from "vitest";

const retryAfter = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api/rate-limit", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api/rate-limit")>();
  return { ...actual, rateLimitRetryAfter: retryAfter };
});

const fetchWithTimeout = vi.hoisted(() => vi.fn());
vi.mock("@/lib/api/fetch", () => ({ fetchWithTimeout }));
vi.mock("@/lib/env", () => ({
  env: { SERVER_URL: "http://server:8082", SERVICE_SECRET: "s3cret" },
}));

import { NextRequest } from "next/server";

import { DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT } from "./route";

function req(
  authorization?: string,
  init: {
    path?: string;
    method?: string;
    body?: BodyInit;
    search?: string;
  } = {},
) {
  const headers = new Headers({ "x-forwarded-for": "203.0.113.9, 10.0.0.1" });
  if (authorization) headers.set("authorization", authorization);
  const path = init.path ?? "organization";
  return new NextRequest(`https://app.example/v1/${path}${init.search ?? ""}`, {
    headers,
    method: init.method ?? "GET",
    body: init.body,
  });
}
const params = { params: Promise.resolve({ path: ["organization"] }) };

beforeEach(() => {
  retryAfter.mockReset().mockResolvedValue(null);
  fetchWithTimeout.mockReset().mockImplementation(
    async () =>
      new Response('{"ok":true}', {
        status: 200,
        headers: { "content-type": "application/json" },
      }),
  );
});

describe("/v1 metering", () => {
  it("charges the source AND the token", async () => {
    await GET(req("Bearer telmoni_abc"), params);
    const keys = retryAfter.mock.calls.map((c) => c[0] as string);
    expect(keys).toHaveLength(2);
    expect(keys[0]).toMatch(/^bfrl:v1:203\.0\.113\.9$/);
    expect(keys[1]).toMatch(/^bfrl:v1:token:[0-9a-f]{32}$/);
  });

  it("charges the source FIRST", async () => {
    await GET(req("Bearer telmoni_abc"), params);
    expect((retryAfter.mock.calls[0]![0] as string)).toContain("bfrl:v1:203");
  });

  it("refuses BEFORE the upstream hop", async () => {
    retryAfter.mockResolvedValueOnce(7);
    const res = await GET(req("Bearer telmoni_abc"), params);
    expect(res.status).toBe(429);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers 429 as problem+json, matching TenantError::RateLimited", async () => {
    retryAfter.mockResolvedValueOnce(7);
    const res = await GET(req("Bearer telmoni_abc"), params);
    expect(res.headers.get("content-type")).toContain("application/problem+json");
    expect(res.headers.get("retry-after")).toBe("7");
    expect(await res.json()).toMatchObject({
      type: "/errors/tenant/rate-limited",
      title: "rate limited",
      status: 429,
      retry_after_secs: 7,
    });
  });

  it("charges a bearer-less request to its address only", async () => {
    await GET(req(), params);
    const keys = retryAfter.mock.calls.map((c) => c[0] as string);
    expect(keys).toEqual(["bfrl:v1:203.0.113.9"]);
  });

  it("never puts the raw bearer in the bucket key", async () => {
    await GET(req("Bearer telmoni_supersecret"), params);
    for (const [key] of retryAfter.mock.calls) {
      expect(key as string).not.toContain("telmoni_supersecret");
      expect(key as string).not.toContain("Bearer");
    }
  });

  it("gives two different bearers two different buckets", async () => {
    await GET(req("Bearer telmoni_one"), params);
    await GET(req("Bearer telmoni_two"), params);
    const tokenKeys = retryAfter.mock.calls
      .map((c) => c[0] as string)
      .filter((k) => k.startsWith("bfrl:v1:token:"));
    expect(tokenKeys).toHaveLength(2);
    expect(tokenKeys[0]).not.toBe(tokenKeys[1]);
  });

  it("gives the same bearer a stable bucket across requests", async () => {
    await GET(req("Bearer telmoni_same"), params);
    await GET(req("Bearer telmoni_same"), params);
    const tokenKeys = retryAfter.mock.calls
      .map((c) => c[0] as string)
      .filter((k) => k.startsWith("bfrl:v1:token:"));
    expect(tokenKeys[0]).toBe(tokenKeys[1]);
  });
});

describe("/v1 routing", () => {
  it("sends a read to auth, as a GET", async () => {
    await GET(req("Bearer telmoni_abc"), params);
    const [target, init] = fetchWithTimeout.mock.calls[0]!;
    expect(target).toBe("http://server:8082/v1/organization");
    expect((init as RequestInit).method).toBe("GET");
  });

  it("carries the query string, so a search survives the hop", async () => {
    await GET(
      req("Bearer telmoni_abc", { path: "organization", search: "?since=2026-01-01" }),
      params,
    );
    const [target] = fetchWithTimeout.mock.calls[0]!;
    expect(target).toBe("http://server:8082/v1/organization?since=2026-01-01");
  });

  it("lets a HEAD reach an auth lane, because it is a read", async () => {
    await HEAD(req("Bearer telmoni_abc", { method: "HEAD" }), params);
    const [target] = fetchWithTimeout.mock.calls[0]!;
    expect(target).toBe("http://server:8082/v1/organization");
  });

  it("refuses a write on an auth lane with 405, before any hop", async () => {
    const res = await POST(
      req("Bearer telmoni_abc", { path: "organization", method: "POST", body: "{}" }),
      params,
    );
    expect(res.status).toBe(405);
    expect(res.headers.get("allow")).toBe("GET, HEAD, OPTIONS");
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  // ⚠ **`AUTH_WRITES` is empty now, and this is what says so out loud.**
  // Every `/v1` lane auth serves is a read. Paths take the default read-only
  // answer, not a stale write entry.
  it.each([
    [["device", "claim"], "POST"],
    [["device"], "DELETE"],
  ])("refuses the retired %s lane before the hop", async (path, method) => {
    const handler = method === "POST" ? POST : DELETE;
    const res = await handler(
      req("Bearer telmoni_abc", { path: path.join("/"), method, body: "{}" }),
      { params: Promise.resolve({ path }) },
    );
    expect(res.status).toBe(405);
    expect(res.headers.get("allow")).toBe("GET, HEAD, OPTIONS");
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("takes no lane from the write table's prototype", async () => {
    const res = await DELETE(
      req("Bearer telmoni_abc", { path: "constructor", method: "DELETE" }),
      { params: Promise.resolve({ path: ["constructor"] }) },
    );
    expect(res.status).toBe(405);
    expect(res.headers.get("allow")).toBe("GET, HEAD, OPTIONS");
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  // A method the route did not export got Next's bodiless 405, with no
  // `allow`: the one `/v1` answer that was no problem document.
  it.each([
    ["PUT", PUT],
    ["PATCH", PATCH],
  ])("answers %s with the same 405 problem document as a POST", async (method, handler) => {
    const res = await handler(
      req("Bearer telmoni_abc", { method, body: "{}" }),
      params,
    );
    expect(res.status).toBe(405);
    expect(res.headers.get("allow")).toBe("GET, HEAD, OPTIONS");
    expect(res.headers.get("content-type")).toBe("application/problem+json");
    expect(((await res.json()) as { type: string }).type).toBe("/errors/method-not-allowed");
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers OPTIONS with the reads it takes, not every method it exports", async () => {
    const res = await OPTIONS();
    expect(res.status).toBe(204);
    expect(res.headers.get("allow")).toBe("GET, HEAD, OPTIONS");
  });

  it("forwards the 401 challenge and a Retry-After, and nothing else of auth's", async () => {
    fetchWithTimeout.mockResolvedValueOnce(
      new Response('{"type":"/errors/auth/invalid-token"}', {
        status: 401,
        headers: {
          "content-type": "application/problem+json",
          "www-authenticate": "Bearer",
          "retry-after": "30",
          "x-request-id": "internal",
        },
      }),
    );
    const res = await GET(req("Bearer telmoni_abc"), params);
    expect(res.status).toBe(401);
    expect(res.headers.get("www-authenticate")).toBe("Bearer");
    expect(res.headers.get("retry-after")).toBe("30");
    expect(res.headers.get("x-request-id")).toBeNull();
  });

  it("answers an unreachable server with a 503 problem document", async () => {
    fetchWithTimeout.mockRejectedValueOnce(new Error("connect ECONNREFUSED"));
    const res = await GET(req("Bearer telmoni_abc"), params);
    expect(res.status).toBe(503);
    expect(res.headers.get("content-type")).toBe("application/problem+json");
    expect(await res.json()).toMatchObject({
      type: "/errors/upstream-unavailable",
      status: 503,
    });
  });

  it("passes the upstream body back untouched", async () => {
    const bytes = new Uint8Array([0x00, 0xff, 0x89, 0x50, 0x0a, 0x1a]);
    fetchWithTimeout.mockResolvedValueOnce(
      new Response(bytes, {
        status: 200,
        headers: { "content-type": "application/octet-stream" },
      }),
    );
    const res = await GET(req("Bearer telmoni_abc"), params);
    expect(new Uint8Array(await res.arrayBuffer())).toEqual(bytes);
  });

  it("never forwards a console context header a client sets", async () => {
    const r = req("Bearer telmoni_abc");
    r.headers.set("x-project-id", "somebody-else");
    r.headers.set("x-organization-id", "somebody-else");
    await GET(r, params);
    const init = fetchWithTimeout.mock.calls[0]![1] as RequestInit;
    const sent = new Headers(init.headers);
    expect(sent.get("x-project-id")).toBeNull();
    expect(sent.get("x-organization-id")).toBeNull();
    expect(sent.get("x-service-secret")).toBe("s3cret");
  });
});
