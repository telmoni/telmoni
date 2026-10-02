import type Redis from "ioredis";
import type { Callback, Result } from "ioredis";
import { NextRequest, NextResponse } from "next/server";

import { getRedis } from "@/lib/redis";
import { logger } from "@/lib/logger";

declare module "ioredis" {
  interface RedisCommander<Context> {
    slidingWindow(
      key: string,
      windowMs: string,
      limit: string,
      member: string,
      callback?: Callback<[number, number]>,
    ): Result<[number, number], Context>;
  }
}

export interface RateLimitOptions {
  limit:    number;
  windowMs: number;
}

export async function rateLimit(
  key:  string,
  opts: RateLimitOptions,
): Promise<NextResponse | null> {
  const retryAfter = await rateLimitRetryAfter(key, opts);
  return retryAfter === null ? null : rateLimitedResponse(retryAfter);
}

export async function rateLimitRetryAfter(
  key:  string,
  opts: RateLimitOptions,
): Promise<number | null> {
  const redis = getRedis();
  if (redis) {
    await firstConnect(redis);
    if (redis.status === "ready") {
      try {
        const [allowed, retryMs] = await withScript(redis).slidingWindow(
          key,
          String(opts.windowMs),
          String(opts.limit),
          crypto.randomUUID(),
        );
        if (allowed === 1) return null;
        return Math.max(1, Math.ceil(retryMs / 1000));
      } catch (err: unknown) {
        logger.error(
          { err },
          "rate-limit: redis error, falling back to the per-instance limiter",
        );
      }
    } else {
      noteFallback(redis.status);
    }
  }
  return inMemoryRateLimit(key, opts);
}

// Registers the script as a command once per client, which is what makes
// every call after the first an EVALSHA carrying a 40-byte digest instead of
// an EVAL carrying the whole script. ioredis tracks that per SOCKET, not per
// client: the first command down a new connection is sent as a full EVAL, so
// a reconnect or a failover onto a node with an empty script cache re-primes
// itself, and a NOSCRIPT that slips through is retried as an EVAL rather than
// surfaced. Hand-rolling EVALSHA is what gets this wrong.
const scripted = new WeakSet<Redis>();

function withScript(redis: Redis): Redis {
  if (!scripted.has(redis)) {
    redis.defineCommand("slidingWindow", {
      numberOfKeys: 1,
      lua: SLIDING_WINDOW_LUA,
    });
    scripted.add(redis);
  }
  return redis;
}

// The client is built by the first request that asks for it, so that request
// always finds it mid-handshake and would take the per-instance limiter (and
// log the fallback) for want of a few milliseconds. Each client gets one
// bounded wait for its first `ready`, shared by whatever arrives during it;
// after that a client that is not ready is down, and the fallback is
// immediate as the options in `lib/redis.ts` intend.
const FIRST_CONNECT_WAIT_MS = 500;
const firstConnects = new WeakMap<Redis, Promise<void>>();

function firstConnect(redis: Redis): Promise<void> {
  let pending = firstConnects.get(redis);
  if (!pending) {
    pending =
      redis.status === "ready"
        ? Promise.resolve()
        : new Promise<void>((resolve) => {
            const done = () => {
              clearTimeout(timer);
              redis.off("ready", done);
              redis.off("end", done);
              resolve();
            };
            const timer = setTimeout(done, FIRST_CONNECT_WAIT_MS);
            redis.once("ready", done);
            redis.once("end", done);
          });
    firstConnects.set(redis, pending);
  }
  return pending;
}

// A client that exists but is not ready falls through to the per-instance
// limiter below with no error to catch, so nothing logged it: every ceiling
// silently became "times the replica count". Say so, but not per request —
// under load that is the log line that drowns the one that explains it.
const FALLBACK_LOG_INTERVAL_MS = 60_000;
let lastFallbackLog = 0;

function noteFallback(status: string) {
  const now = Date.now();
  if (now - lastFallbackLog < FALLBACK_LOG_INTERVAL_MS) return;
  lastFallbackLog = now;
  logger.warn(
    { status },
    "rate-limit: redis is not ready, using the per-instance limiter",
  );
}

function rateLimitedResponse(retryAfterSecs: number): NextResponse {
  return NextResponse.json(
    { error: "rate limit exceeded" },
    {
      status:  429,
      headers: { "retry-after": String(retryAfterSecs), "cache-control": "no-store" },
    },
  );
}

export function sessionKey(
  session: { userId: string },
  action:  string,
): string {
  return `bfrl:${session.userId}:${action}`;
}

export function clientKey(request: NextRequest, scope: string): string {
  return clientKeyFromHeaders(request.headers, scope);
}

// The same bucket from a Server Action, which has no request object: the
// pre-session account actions (sign-in, sign-up, the reset) meter the caller
// by address, since there is no session to meter them by.
export function clientKeyFromHeaders(headers: Headers, scope: string): string {
  return `bfrl:${scope}:${trustedClientIp(headers)}`;
}

// ⚠ **`X-Forwarded-For` and nothing a caller can write.** Google's load
// balancer appends the address it saw the client at, then its own, so the
// second entry from the end is the one hop a caller cannot forge. A header
// another vendor's edge would set (`cf-connecting-ip`, `true-client-ip`) is
// just a request header here: read first, it let any caller pick a fresh
// bucket per request and walk past every per-address ceiling.
function trustedClientIp(headers: Headers): string {
  const parts = headers
    .get("x-forwarded-for")
    ?.split(",")
    .map((p) => p.trim())
    .filter(Boolean);
  if (parts && parts.length >= 2) return parts[parts.length - 2]!;
  if (parts && parts.length === 1) return parts[0]!;
  return headers.get("x-real-ip") || "unknown";
}

const SLIDING_WINDOW_LUA = `
-- ⚠ **The window is measured by the SERVER's clock, and the caller does not
-- get a vote.** One bucket is shared by every console replica, so a caller's
-- own Date.now() writes scores that its neighbours then read and subtract
-- from: an instance whose clock runs ahead lands entries in their future,
-- where nobody's cleanup can reach them, and one whose clock runs behind has
-- its own entries swept early. Reading the clock here makes the bucket
-- internally consistent no matter how many instances share it.
--
-- Allowed because Redis replicates a script's EFFECTS rather than the script:
-- the replica applies the resulting ZADD instead of re-running TIME and
-- reaching a different answer. That has been the default since Redis 5 and
-- this deploys on 7.
local t      = redis.call("TIME")
local now    = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
local window = tonumber(ARGV[1])
local limit  = tonumber(ARGV[2])
redis.call("ZREMRANGEBYSCORE", KEYS[1], 0, now - window)
local count = redis.call("ZCARD", KEYS[1])
if count >= limit then
  -- The window may be EMPTY here, and the arithmetic below must not assume
  -- otherwise. At limit <= 0 the branch is taken with nothing stored, so
  -- oldest[2] is nil and tonumber(nil) raised a Lua error -- which the
  -- caller catches as "Redis went sideways" and answers by falling through
  -- to the in-memory limiter, which ADMITS the request. A limit of zero is
  -- the most restrictive setting there is, so it was the one that failed
  -- open. Fall back to the full window instead.
  -- (No backticks in this comment: it lives inside a JS template literal.)
  local oldest = redis.call("ZRANGE", KEYS[1], 0, 0, "WITHSCORES")
  local retry_ms = window
  if oldest[2] ~= nil then
    retry_ms = window - (now - tonumber(oldest[2]))
  end
  return {0, retry_ms}
end
redis.call("ZADD", KEYS[1], now, ARGV[3])
redis.call("PEXPIRE", KEYS[1], window)
return {1, 0}
`;

interface Bucket {
  hits: number[];
}

const buckets = new Map<string, Bucket>();
let lastSweep = 0;

function sweepIfStale(now: number) {
  if (now - lastSweep < 60_000) return;
  lastSweep = now;
  for (const [key, bucket] of buckets) {
    if (
      bucket.hits.length === 0 ||
      now - bucket.hits[bucket.hits.length - 1]! > 600_000
    ) {
      buckets.delete(key);
    }
  }
}

function inMemoryRateLimit(
  key:  string,
  opts: RateLimitOptions,
): number | null {
  const now = Date.now();
  sweepIfStale(now);

  const bucket = buckets.get(key) ?? { hits: [] };
  bucket.hits = bucket.hits.filter((t) => now - t < opts.windowMs);

  if (bucket.hits.length >= opts.limit) {
    const oldest = bucket.hits[0];
    return oldest === undefined
      ? Math.ceil(opts.windowMs / 1000)
      : Math.max(1, Math.ceil((opts.windowMs - (now - oldest)) / 1000));
  }

  bucket.hits.push(now);
  buckets.set(key, bucket);
  return null;
}
