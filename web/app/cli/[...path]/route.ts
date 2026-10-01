import { type NextRequest, NextResponse } from "next/server";
import { z } from "zod";

import { readBodyCapped } from "@/lib/api/body";
import { fetchWithTimeout } from "@/lib/api/fetch";
import { clientKey, rateLimitRetryAfter } from "@/lib/api/rate-limit";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

// ⚠ **The door a native client (the CLI) enters by, and a security boundary
// in the same sense `/v1` is.** Every auth lane sits behind the service
// secret, which only this process holds, so nothing outside the platform can
// reach auth except through a Route Handler that chooses to forward. This one
// forwards exactly the lanes below and nothing else: a path that is not in
// the table is a 404 before any hop, a body is parsed here and re-encoded so
// only the fields a lane takes cross, and a bearer travels only on the lanes
// that act for a person. The bearer itself is the person's own provider
// token, verified by auth against the provider's keys — this door adds no
// identity of its own and never reads a cookie.
//
// The lanes are the CLI's session lifecycle and no more: begin a device
// authorization, poll it, refresh, `/me` (which records the session row that
// puts the CLI on the Active sessions page), and end one's own session. A
// command that reads or writes organization data is a lane to add HERE, on
// purpose, not a catch-all to widen.

const UPSTREAM_TIMEOUT_MS = 10_000;
const BODY_CAP_BYTES = 8 * 1024;
const PER_SOURCE = { limit: 120, windowMs: 60_000 };
const PER_BEARER = { limit: 600, windowMs: 60_000 };

// Wire shapes are camelCase at this door. Auth's own lanes are not uniform
// (`/me` and the device lanes are camelCase; refresh is snake_case), and the
// translation belongs here rather than in every client.
const PollBody = z.object({
  deviceCode: z.string().min(1).max(512),
});
const RefreshBody = z.object({
  refreshToken: z.string().min(1).max(4096),
  sessionRowId: z.guid().optional(),
});

type Lane = {
  // The door's path, segment by segment; `":id"` takes one UUID.
  pattern: readonly string[];
  // Whether the lane acts for a person: the bearer and `x-organization-id`
  // are forwarded, and the bearer is metered.
  bearer: boolean;
  // The auth path, given the UUID segments in order.
  upstream: (ids: string[]) => string;
  // What the hop carries as its body.
  body:
    | { kind: "none" }
    | { kind: "user-agent" }
    | { kind: "json"; schema: z.ZodType; encode: (value: unknown) => unknown };
};

const LANES: readonly Lane[] = [
  {
    pattern: ["auth", "device"],
    bearer: false,
    upstream: () => "/internal/auth/device/start",
    body: { kind: "none" },
  },
  {
    pattern: ["auth", "device", "poll"],
    bearer: false,
    upstream: () => "/internal/auth/device/poll",
    body: { kind: "json", schema: PollBody, encode: (value) => value },
  },
  {
    pattern: ["auth", "refresh"],
    bearer: false,
    upstream: () => "/internal/auth/refresh",
    body: {
      kind: "json",
      schema: RefreshBody,
      encode: (value) => {
        const { refreshToken, sessionRowId } = value as z.infer<typeof RefreshBody>;
        return { refresh_token: refreshToken, session_row_id: sessionRowId ?? null };
      },
    },
  },
  {
    pattern: ["me"],
    bearer: true,
    upstream: () => "/me",
    body: { kind: "user-agent" },
  },
  {
    pattern: ["sessions", ":id", "revoke"],
    bearer: true,
    upstream: ([id]) => `/internal/auth/sessions/${id}/revoke`,
    body: { kind: "none" },
  },
];

function match(path: string[]): { lane: Lane; ids: string[] } | null {
  for (const lane of LANES) {
    if (lane.pattern.length !== path.length) continue;
    const ids: string[] = [];
    let ok = true;
    for (let i = 0; i < path.length && ok; i++) {
      const want = lane.pattern[i]!;
      const got = path[i]!;
      if (want === ":id") {
        if (z.guid().safeParse(got).success) ids.push(got.toLowerCase());
        else ok = false;
      } else if (want !== got) {
        ok = false;
      }
    }
    if (ok) return { lane, ids };
  }
  return null;
}

function problem(
  status: number,
  type: string,
  title: string,
  detail: string,
  headers: Record<string, string> = {},
): NextResponse {
  return NextResponse.json(
    { type, title, status, detail },
    {
      status,
      headers: {
        "content-type": "application/problem+json",
        "cache-control": "no-store, private",
        ...headers,
      },
    },
  );
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

async function bearerBucket(authorization: string): Promise<string> {
  const digest = await crypto.subtle.digest(
    "SHA-256",
    new TextEncoder().encode(authorization),
  );
  const hex = Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return `bfrl:cli:token:${hex.slice(0, 32)}`;
}

async function proxy(request: NextRequest, path: string[]): Promise<NextResponse> {
  // A browser sends `Sec-Fetch-Site` on every request and a native client
  // sends none. Nothing here is for a page, and a page that could reach the
  // refresh lane with a stored token would be a session it never held.
  const secFetchSite = request.headers.get("sec-fetch-site");
  if (secFetchSite && secFetchSite !== "none") {
    return problem(403, "/errors/authz/forbidden", "forbidden", "not a browser lane");
  }

  const matched = match(path);
  if (!matched) {
    return problem(404, "/errors/auth/not-found", "not found", "no such lane");
  }
  if (request.method.toUpperCase() !== "POST") {
    return problem(405, "/errors/method-not-allowed", "method not allowed", "this lane takes POST", {
      allow: "POST",
    });
  }
  const { lane, ids } = matched;

  const authorization = request.headers.get("authorization");
  if (lane.bearer && !authorization?.startsWith("Bearer ")) {
    return problem(401, "/errors/auth/unauthenticated", "unauthenticated", "a bearer is required");
  }

  const buckets: [string, { limit: number; windowMs: number }][] = [
    [clientKey(request, "cli"), PER_SOURCE],
  ];
  if (lane.bearer && authorization) buckets.push([await bearerBucket(authorization), PER_BEARER]);
  for (const [bucket, ceiling] of buckets) {
    const retryAfter = await rateLimitRetryAfter(bucket, ceiling);
    if (retryAfter !== null) return rateLimitedProblem(retryAfter);
  }

  let body: string | undefined;
  if (lane.body.kind === "json") {
    const raw = await readBodyCapped(request, BODY_CAP_BYTES);
    if (raw === null) {
      return problem(413, "/errors/payload-too-large", "payload too large", "body over 8 KiB");
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(new TextDecoder().decode(raw));
    } catch {
      return problem(400, "/errors/bad-request", "bad request", "body is not JSON");
    }
    const checked = lane.body.schema.safeParse(parsed);
    if (!checked.success) {
      return problem(400, "/errors/bad-request", "bad request", "body is not the shape this lane takes");
    }
    body = JSON.stringify(lane.body.encode(checked.data));
  } else if (lane.body.kind === "user-agent") {
    body = JSON.stringify({ userAgent: request.headers.get("user-agent") });
  }

  const unavailable = NextResponse.json({ error: "upstream unavailable" }, { status: 503 });
  const base = env.SERVER_URL;

  const headers = new Headers({
    "x-service-secret": env.SERVICE_SECRET,
    "x-request-id": crypto.randomUUID(),
    accept: "application/json",
  });
  if (body !== undefined) headers.set("content-type", "application/json");
  if (lane.bearer) {
    headers.set("authorization", authorization!);
    const organization = request.headers.get("x-organization-id");
    if (organization) headers.set("x-organization-id", organization);
  }

  const target = `${base}${lane.upstream(ids)}`;
  const lanePath = path.join("/");
  let upstream: Response;
  try {
    upstream = await fetchWithTimeout(
      target,
      { method: "POST", headers, body, cache: "no-store", signal: request.signal },
      UPSTREAM_TIMEOUT_MS,
    );
  } catch (err) {
    logger.warn(
      { lane: lanePath, error: err instanceof Error ? err.message : String(err) },
      "cli: upstream fetch failed",
    );
    return unavailable;
  }

  const responseHeaders = new Headers({ "cache-control": "no-store, private" });
  for (const name of ["content-type", "content-length"] as const) {
    const value = upstream.headers.get(name);
    if (value) responseHeaders.set(name, value);
  }
  return new NextResponse(upstream.body, { status: upstream.status, headers: responseHeaders });
}

type Context = { params: Promise<{ path: string[] }> };

export async function GET(request: NextRequest, context: Context): Promise<NextResponse> {
  const { path } = await context.params;
  return proxy(request, path);
}

export async function POST(request: NextRequest, context: Context): Promise<NextResponse> {
  const { path } = await context.params;
  return proxy(request, path);
}

export async function DELETE(request: NextRequest, context: Context): Promise<NextResponse> {
  const { path } = await context.params;
  return proxy(request, path);
}
