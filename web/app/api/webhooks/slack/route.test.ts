import { NextRequest } from "next/server";
import { beforeEach, describe, expect, it, vi } from "vitest";

import * as route from "./route";

vi.mock("@/lib/api/fetch", () => ({
  fetchWithTimeout: vi.fn(),
}));
vi.mock("@/lib/env", () => ({
  env: {
    SERVER_URL: "http://notifications.internal:8086",
    SERVICE_SECRET: "test-service-secret",
  },
}));

const { fetchWithTimeout } = await import("@/lib/api/fetch");
const mockFetch = vi.mocked(fetchWithTimeout);

const { POST } = route;

beforeEach(() => {
  mockFetch.mockReset();
});

const RAW_BODY = '{"type": "event_callback",\t"team_id":"T0001",  "event":{"type":"app_uninstalled"}}';
const RAW_BYTES = new TextEncoder().encode(RAW_BODY);

const SIGNATURE = "v0=a2114d57b48eac39b9ad189dd8316235a7b4a8d21a10bd27519666489c69b503";
const TIMESTAMP = "1531420618";

function eventRequest(overrides?: {
  body?: Uint8Array;
  headers?: Record<string, string>;
  omitSignature?: boolean;
}): NextRequest {
  const headers: Record<string, string> = {
    "content-type": "application/json",
    ...(overrides?.omitSignature
      ? {}
      : { "x-slack-signature": SIGNATURE, "x-slack-request-timestamp": TIMESTAMP }),
    ...overrides?.headers,
  };
  return new NextRequest("http://localhost:3000/api/webhooks/slack", {
    method: "POST",
    headers,
    body: (overrides?.body ?? RAW_BYTES) as unknown as BodyInit,
  });
}

function notificationsAnswers(status: number, body: string) {
  mockFetch.mockResolvedValue(
    new Response(body, { status, headers: { "content-type": "application/json" } }),
  );
}

describe("POST /api/webhooks/slack", () => {
  it("forwards the body BYTES untouched — the signature base", async () => {
    expect(JSON.stringify(JSON.parse(RAW_BODY))).not.toBe(RAW_BODY);

    notificationsAnswers(200, '{"ok":true}');
    await POST(eventRequest());

    expect(mockFetch).toHaveBeenCalledTimes(1);
    const [, init] = mockFetch.mock.calls[0]!;
    const sent = new Uint8Array(init!.body as ArrayBuffer);
    expect(Array.from(sent)).toEqual(Array.from(RAW_BYTES));
  });

  it("forwards the two signature headers and the retry counter verbatim, and NEVER the service secret", async () => {
    notificationsAnswers(200, '{"ok":true}');
    await POST(eventRequest({ headers: { "x-slack-retry-num": "2" } }));

    const [url, init] = mockFetch.mock.calls[0]!;
    expect(url).toBe("http://notifications.internal:8086/webhooks/slack");
    expect(init!.method).toBe("POST");

    const headers = new Headers(init!.headers);
    expect(headers.get("x-slack-signature")).toBe(SIGNATURE);
    expect(headers.get("x-slack-request-timestamp")).toBe(TIMESTAMP);
    expect(headers.get("x-slack-retry-num")).toBe("2");
    expect(headers.get("content-type")).toBe("application/json");
    expect(headers.get("x-request-id")).toMatch(/^[0-9a-f-]{36}$/);
    expect(headers.get("x-service-secret")).toBeNull();
  });

  it("gives the hop less than Slack's three seconds, so a slow service is a retry rather than a silent failure", async () => {
    notificationsAnswers(200, '{"ok":true}');
    await POST(eventRequest());
    const [, , timeout] = mockFetch.mock.calls[0]!;
    expect(timeout).toBeLessThan(3_000);
  });

  it("forwards a missing signature as missing — the service owns the 401", async () => {
    notificationsAnswers(401, '{"title":"unauthenticated"}');
    const res = await POST(eventRequest({ omitSignature: true }));

    const headers = new Headers(mockFetch.mock.calls[0]![1]!.headers);
    expect(headers.get("x-slack-signature")).toBeNull();
    expect(res.status).toBe(401);
  });

  it.each([200, 400, 401, 500])(
    "passes the service's %i status and body through",
    async (status) => {
      notificationsAnswers(status, `{"status":${status}}`);
      const res = await POST(eventRequest());
      expect(res.status).toBe(status);
      expect(await res.text()).toBe(`{"status":${status}}`);
      expect(res.headers.get("cache-control")).toBe("no-store, private");
    },
  );

  it("answers 503 when the service is unreachable — Slack retries a non-2xx", async () => {
    mockFetch.mockRejectedValue(new Error("ECONNREFUSED"));
    const res = await POST(eventRequest());
    expect(res.status).toBe(503);
    expect(await res.text()).not.toContain("notifications.internal");
  });

  // No content-length at all, and more bytes than the ceiling: the case a
  // header check waves through and `arrayBuffer()` would buffer whole.
  it("refuses a chunked body that crosses the ceiling", async () => {
    const chunk = new Uint8Array(512 * 1024);
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        for (let i = 0; i < 8; i++) controller.enqueue(chunk);
        controller.close();
      },
    });
    const res = await POST(
      new NextRequest(
        new Request("http://localhost:3000/api/webhooks/slack", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: stream,
          duplex: "half",
        } as RequestInit),
      ),
    );
    expect(res.status).toBe(413);
    expect(mockFetch).not.toHaveBeenCalled();
  });

  // A sender that goes away mid-upload. The read rejects, and the answer is a
  // 400 the vendor retries rather than a 500 logged at error severity.
  it("answers 400 when the body cannot be read", async () => {
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.error(new Error("client went away"));
      },
    });
    const res = await POST(
      new NextRequest(
        new Request("http://localhost:3000/api/webhooks/slack", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: stream,
          duplex: "half",
        } as RequestInit),
      ),
    );
    expect(res.status).toBe(400);
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it("refuses an oversized body with 413 before reading or forwarding it", async () => {
    const res = await POST(
      eventRequest({ headers: { "content-length": String(2 * 1024 * 1024) } }),
    );
    expect(res.status).toBe(413);
    expect(mockFetch).not.toHaveBeenCalled();
  });

  it("exports POST and nothing else — absent methods are the contract", () => {
    expect(Object.keys(route).sort()).toEqual(["POST"]);
  });
});
