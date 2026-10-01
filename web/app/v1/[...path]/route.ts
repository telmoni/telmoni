import { type NextRequest, NextResponse } from "next/server";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { clientKey, rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

// The client's own Authorization reaches `/v1` only; no console path sets one on a `/v1` hop.
const FORWARDED_REQUEST_HEADERS = [
  "authorization",
  "accept",
  "content-type",
  "content-length",
] as const;

const FORWARDED_RESPONSE_HEADERS = ["content-type", "content-length"] as const;
const READ_TIMEOUT_MS = 10_000;
// ⚠ **Empty, and that is the current truth rather than an oversight.** Every
// `/v1` lane auth serves is a read. A path listed here is forwarded with its
// method instead of answering 405, so an entry for a lane auth does not serve
// is a method this door admits and cannot satisfy —
// `every_auth_write_is_one_the_front_door_admits` asserts both directions.
const AUTH_WRITES: Record<string, string> = {};
const PER_TOKEN = { limit: 600, windowMs: 60_000 };
const PER_SOURCE = { limit: 1_200, windowMs: 60_000 };

async function tokenBucket(authorization: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(authorization),
  );
  const hex = Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return `bfrl:v1:token:${hex.slice(0, 32)}`;
}

function rateLimitedProblem(retryAfterSecs: number): NextResponse {
  return NextResponse.json(
    {
      type: "/errors/tenant/rate-limited",
      title: "rate limited",
      status: 429,
      detail: `retry after ${retryAfterSecs}s`,
      retry_after_secs: retryAfterSecs,
    },
    {
      status: 429,
      headers: {
        "content-type": "application/problem+json",
        "retry-after": String(retryAfterSecs),
        "cache-control": "no-store, private",
      },
    },
  );
}

function methodNotAllowed(allow: string): NextResponse {
  return NextResponse.json(
    {
      type: "/errors/method-not-allowed",
      title: "method not allowed",
      status: 405,
      detail: `this lane takes ${allow}`,
    },
    {
      status: 405,
      headers: {
        "content-type": "application/problem+json",
        allow,
        "cache-control": "no-store, private",
      },
    },
  );
}

async function proxy(
  request: NextRequest,
  path: string[],
): Promise<NextResponse> {
  const base = env.SERVER_URL;
  const method = request.method.toUpperCase();
  const lane = path.join("/");
  const write = Object.hasOwn(AUTH_WRITES, lane) ? AUTH_WRITES[lane] : undefined;
  if (write) {
    if (method !== write) return methodNotAllowed(write);
  } else if (method !== "GET" && method !== "HEAD") {
    return methodNotAllowed("GET, HEAD");
  }

  const authorization = request.headers.get("authorization");
  const buckets: [string, { limit: number; windowMs: number }][] = [
    [clientKey(request, "v1"), PER_SOURCE],
  ];
  if (authorization) buckets.push([await tokenBucket(authorization), PER_TOKEN]);
  for (const [bucket, ceiling] of buckets) {
    const retryAfter = await rateLimitRetryAfter(bucket, ceiling);
    if (retryAfter !== null) return rateLimitedProblem(retryAfter);
  }

  const unavailable = NextResponse.json(
    { error: "upstream unavailable" },
    { status: 503 },
  );

  const suffix = path.map(encodeURIComponent).join("/");
  const target = `${base}/v1/${suffix}${request.nextUrl.search}`;

  const headers = new Headers();
  for (const name of FORWARDED_REQUEST_HEADERS) {
    const value = request.headers.get(name);
    if (value) headers.set(name, value);
  }
  headers.set("x-service-secret", env.SERVICE_SECRET);
  const requestId = crypto.randomUUID();
  headers.set("x-request-id", requestId);

  // `request.signal`: a caller that disconnects cancels the hop upstream
  // rather than leaving auth to finish a request nobody is waiting for.
  const init: RequestInit & { duplex?: "half" } = {
    method,
    headers,
    cache: "no-store",
    signal: request.signal,
  };
  // Unreachable while `AUTH_WRITES` is empty — the method check above answers
  // 405 first — and kept deliberately: it is the general handling for the
  // `write` branch, not device-specific. Deleting it means the next write lane
  // added to the table forwards with its body silently dropped.
  if (method === "POST" && request.body) {
    init.body = request.body;
    init.duplex = "half";
  }

  let upstream: Response;
  try {
    upstream = await fetchWithTimeout(target, init, READ_TIMEOUT_MS);
  } catch (err) {
    // A 503 with no line behind it is an outage nobody can see from the logs.
    // The request id is what ties this to auth's own record of the same hop.
    logger.warn(
      { lane, requestId, error: err instanceof Error ? err.message : String(err) },
      "v1: upstream fetch failed",
    );
    return unavailable;
  }

  const responseHeaders = new Headers();
  for (const name of FORWARDED_RESPONSE_HEADERS) {
    const value = upstream.headers.get(name);
    if (value) responseHeaders.set(name, value);
  }
  responseHeaders.set("cache-control", "no-store, private");

  return new NextResponse(upstream.body, {
    status: upstream.status,
    headers: responseHeaders,
  });
}

type Context = { params: Promise<{ path: string[] }> };

export async function GET(
  request: NextRequest,
  context: Context,
): Promise<NextResponse> {
  const { path } = await context.params;
  return proxy(request, path);
}

export async function POST(
  request: NextRequest,
  context: Context,
): Promise<NextResponse> {
  const { path } = await context.params;
  return proxy(request, path);
}

export async function DELETE(
  request: NextRequest,
  context: Context,
): Promise<NextResponse> {
  const { path } = await context.params;
  return proxy(request, path);
}

export async function HEAD(
  request: NextRequest,
  context: Context,
): Promise<NextResponse> {
  const { path } = await context.params;
  return proxy(request, path);
}

