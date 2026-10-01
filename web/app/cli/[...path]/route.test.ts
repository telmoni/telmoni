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

import { DELETE, GET, POST } from "./route";

const ROW = "0192a3b4-c5d6-7e8f-9a0b-1c2d3e4f5a6b";
const BEARER = "Bearer eyJ.person.token";

function req(
  path: string,
  init: {
    method?: string;
    body?: unknown;
    headers?: Record<string, string>;
  } = {},
) {
  const headers = new Headers({ "x-forwarded-for": "203.0.113.9, 10.0.0.1" });
  for (const [name, value] of Object.entries(init.headers ?? {})) headers.set(name, value);
  const body = init.body === undefined ? undefined : JSON.stringify(init.body);
  if (body !== undefined) headers.set("content-type", "application/json");
  return new NextRequest(`https://app.example/cli/${path}`, {
    headers,
    method: init.method ?? "POST",
    body,
  });
}

function call(path: string, init: Parameters<typeof req>[1] = {}) {
  const handler = init.method === "GET" ? GET : init.method === "DELETE" ? DELETE : POST;
  return handler(req(path, init), { params: Promise.resolve({ path: path.split("/") }) });
}

function sentInit(): RequestInit & { headers: Headers } {
  const init = fetchWithTimeout.mock.calls[0]![1] as RequestInit;
  return { ...init, headers: new Headers(init.headers) };
}

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

describe("/cli lanes", () => {
  it("sends a device start to auth with no body, whatever the client sent", async () => {
    const res = await call("auth/device", { body: { anything: "ignored" } });
    expect(res.status).toBe(200);
    const [target] = fetchWithTimeout.mock.calls[0]!;
    expect(target).toBe("http://server:8082/internal/auth/device/start");
    const init = sentInit();
    expect(init.method).toBe("POST");
    expect(init.body).toBeUndefined();
    expect(init.headers.get("x-service-secret")).toBe("s3cret");
    expect(init.headers.get("x-request-id")).toMatch(/[0-9a-f-]{36}/);
  });

  it("sends a poll to auth's device poll, body checked and re-encoded", async () => {
    await call("auth/device/poll", { body: { deviceCode: "dc_1", extra: "dropped" } });
    const [target] = fetchWithTimeout.mock.calls[0]!;
    expect(target).toBe("http://server:8082/internal/auth/device/poll");
    expect(JSON.parse(sentInit().body as string)).toEqual({ deviceCode: "dc_1" });
  });

  it("passes a pending poll back as the 202 auth answered", async () => {
    fetchWithTimeout.mockResolvedValueOnce(
      new Response('{"status":"authorization_pending"}', {
        status: 202,
        headers: { "content-type": "application/json" },
      }),
    );
    const res = await call("auth/device/poll", { body: { deviceCode: "dc_1" } });
    expect(res.status).toBe(202);
    expect(await res.json()).toEqual({ status: "authorization_pending" });
  });

  it("translates a refresh into auth's snake_case body", async () => {
    await call("auth/refresh", { body: { refreshToken: "rt_1", sessionRowId: ROW } });
    const [target] = fetchWithTimeout.mock.calls[0]!;
    expect(target).toBe("http://server:8082/internal/auth/refresh");
    expect(JSON.parse(sentInit().body as string)).toEqual({ refresh_token: "rt_1", session_row_id: ROW });
  });

  it("sends a refresh with no row as a null row, which auth takes as absent", async () => {
    await call("auth/refresh", { body: { refreshToken: "rt_1" } });
    expect(JSON.parse(sentInit().body as string)).toEqual({ refresh_token: "rt_1", session_row_id: null });
  });

  it("sends /me with the client's user agent as its whole body", async () => {
    await call("me", { headers: { authorization: BEARER, "user-agent": "telmoni-cli/0.0.1 (macos; aarch64)" } });
    const [target] = fetchWithTimeout.mock.calls[0]!;
    expect(target).toBe("http://server:8082/me");
    expect(JSON.parse(sentInit().body as string)).toEqual({ userAgent: "telmoni-cli/0.0.1 (macos; aarch64)" });
  });

  it("ignores any body a client sends to /me, so nothing in it can describe the person", async () => {
    await call("me", { headers: { authorization: BEARER }, body: { email: "someone-else@example.com" } });
    expect(JSON.parse(sentInit().body as string)).toEqual({ userAgent: null });
  });

  it("sends a revoke to the person lane by row id, with no body", async () => {
    await call(`sessions/${ROW.toUpperCase()}/revoke`, { headers: { authorization: BEARER } });
    const [target] = fetchWithTimeout.mock.calls[0]!;
    expect(target).toBe(`http://server:8082/internal/auth/sessions/${ROW}/revoke`);
    expect(sentInit().body).toBeUndefined();
  });
});

describe("/cli refusals, all before the hop", () => {
  it("answers 404 for a lane not in the table", async () => {
    for (const path of [
      "projects",
      "auth",
      "auth/logout",
      "auth/start",
      "auth/exchange",
      "auth/device/extra",
      "sessions/revoke",
      "me/extra",
      "constructor",
    ]) {
      const res = await call(path, { headers: { authorization: BEARER }, body: {} });
      expect(res.status, path).toBe(404);
    }
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers 404 for a revoke whose id is not a uuid", async () => {
    const res = await call("sessions/not-a-uuid/revoke", { headers: { authorization: BEARER } });
    expect(res.status).toBe(404);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers 405 with Allow: POST for any other method", async () => {
    for (const method of ["GET", "DELETE"]) {
      const res = await call("me", { method, headers: { authorization: BEARER } });
      expect(res.status, method).toBe(405);
      expect(res.headers.get("allow")).toBe("POST");
    }
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers 401 on a person lane with no bearer", async () => {
    for (const path of ["me", `sessions/${ROW}/revoke`]) {
      const res = await call(path);
      expect(res.status, path).toBe(401);
      expect(await res.json()).toMatchObject({ type: "/errors/auth/unauthenticated" });
    }
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers 400 for a body that is not JSON or not the lane's shape", async () => {
    const notJson = new NextRequest("https://app.example/cli/auth/refresh", {
      method: "POST",
      headers: { "content-type": "application/json", "x-forwarded-for": "203.0.113.9" },
      body: "{not json",
    });
    const res1 = await POST(notJson, { params: Promise.resolve({ path: ["auth", "refresh"] }) });
    expect(res1.status).toBe(400);

    const res2 = await call("auth/refresh", { body: { refresh_token: "snake-case-is-not-the-contract" } });
    expect(res2.status).toBe(400);

    const res3 = await call("auth/refresh", { body: { refreshToken: "rt", sessionRowId: "not-a-uuid" } });
    expect(res3.status).toBe(400);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("answers 413 for a body over the cap", async () => {
    const res = await call("auth/device/poll", { body: { deviceCode: "c".repeat(9000) } });
    expect(res.status).toBe(413);
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("refuses a request a browser made, whatever its origin", async () => {
    for (const site of ["same-origin", "same-site", "cross-site"]) {
      const res = await call("auth/refresh", {
        headers: { "sec-fetch-site": site },
        body: { refreshToken: "rt_1" },
      });
      expect(res.status, site).toBe(403);
    }
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });

  it("takes a request whose Sec-Fetch-Site is none, which a typed address sends", async () => {
    const res = await call("auth/refresh", { headers: { "sec-fetch-site": "none" }, body: { refreshToken: "rt_1" } });
    expect(res.status).toBe(200);
  });

  it("answers 503 when the hop fails, and says which lane in the log line only", async () => {
    fetchWithTimeout.mockRejectedValueOnce(new Error("boom"));
    const res = await call("auth/refresh", { body: { refreshToken: "rt_1" } });
    expect(res.status).toBe(503);
    expect(await res.text()).not.toContain("boom");
  });
});

describe("/cli headers", () => {
  it("forwards the bearer and x-organization-id on a person lane and never a project id", async () => {
    await call("me", {
      headers: { authorization: BEARER, "x-organization-id": "org_abc", "x-project-id": "proj_abc" },
    });
    const sent = sentInit().headers;
    expect(sent.get("authorization")).toBe(BEARER);
    expect(sent.get("x-organization-id")).toBe("org_abc");
    expect(sent.get("x-project-id")).toBeNull();
  });

  it("forwards no bearer and no organization on a pre-session lane, even when sent", async () => {
    await call("auth/refresh", {
      headers: { authorization: BEARER, "x-organization-id": "org_abc" },
      body: { refreshToken: "rt_1" },
    });
    const sent = sentInit().headers;
    expect(sent.get("authorization")).toBeNull();
    expect(sent.get("x-organization-id")).toBeNull();
    expect(sent.get("x-service-secret")).toBe("s3cret");
  });

  it("passes the upstream status and body back, so auth's refusal is the client's answer", async () => {
    fetchWithTimeout.mockResolvedValueOnce(
      new Response('{"type":"/errors/auth/unauthenticated","status":401}', {
        status: 401,
        headers: { "content-type": "application/problem+json", "x-internal": "hidden" },
      }),
    );
    const res = await call("me", { headers: { authorization: BEARER } });
    expect(res.status).toBe(401);
    expect(res.headers.get("content-type")).toBe("application/problem+json");
    expect(res.headers.get("x-internal")).toBeNull();
    expect(res.headers.get("cache-control")).toBe("no-store, private");
    expect(await res.json()).toMatchObject({ type: "/errors/auth/unauthenticated" });
  });
});

describe("/cli metering", () => {
  it("charges the source on every lane, and the bearer on a person lane", async () => {
    await call("auth/refresh", { body: { refreshToken: "rt_1" } });
    expect(retryAfter.mock.calls.map((c) => c[0])).toEqual(["bfrl:cli:203.0.113.9"]);

    retryAfter.mockClear();
    await call("me", { headers: { authorization: BEARER } });
    const keys = retryAfter.mock.calls.map((c) => c[0] as string);
    expect(keys[0]).toBe("bfrl:cli:203.0.113.9");
    expect(keys[1]).toMatch(/^bfrl:cli:token:[0-9a-f]{32}$/);
    expect(keys[1]).not.toContain("person.token");
  });

  it("refuses with a 429 problem before the hop", async () => {
    retryAfter.mockResolvedValueOnce(9);
    const res = await call("me", { headers: { authorization: BEARER } });
    expect(res.status).toBe(429);
    expect(res.headers.get("retry-after")).toBe("9");
    expect(await res.json()).toMatchObject({ type: "/errors/tenant/rate-limited", retry_after_secs: 9 });
    expect(fetchWithTimeout).not.toHaveBeenCalled();
  });
});
