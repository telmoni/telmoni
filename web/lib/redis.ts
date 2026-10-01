import Redis, { type RedisOptions } from "ioredis";
import { env } from "./env";
import { logger } from "./logger";

let client: Redis | null = null;
let constructed = false;

// ⚠ **The OS has to probe, because our quietest connection is the one we
// cannot afford to lose.** ioredis defaults `keepAlive` to 0, which is off:
// the socket is only discovered dead when something writes to it. The event
// subscriber writes nothing for as long as nobody invites anybody, and Google
// Cloud's VPC firewall stops tracking a connection that carries no packet for
// ten minutes, after which what Redis sends down it can be refused — so
// without probes the stream stays open, the client believes it is subscribed,
// and no message ever arrives again. The server's own `tcp-keepalive` may
// cover this, but it is Memorystore's setting and not ours to depend on.
const KEEPALIVE_MS = 30_000;

// ⚠ **The one failure the other options do not cover: connected, and not
// answering.** `maxRetriesPerRequest` and `enableOfflineQueue: false` make a
// KNOWN-down Redis fail immediately, which is what every caller's fallback is
// written for. Neither bounds a command sent to a server that holds the socket
// open and never replies — a node mid-failover, a black-holed route, an
// instance stalled on its own snapshot. That command hangs forever, and the
// limiter is awaited inline in Server Actions and in the `/v1` proxy, so the
// request hangs with it. A timeout turns that into the error the callers
// already handle. Generous on purpose: this is a circuit breaker for a stalled
// server, not a latency budget, and a value tight enough to trip on a slow-ish
// round trip would quietly weaken every ceiling it fell back from.
const COMMAND_TIMEOUT_MS = 1_000;

// ⚠ **A Memorystore failover can leave us writing to a replica, and ioredis
// will not notice on its own.** `STANDARD_HA` promotes the standby behind the
// same endpoint; a connection held across that can end up on the demoted node,
// where every write — the limiter's `ZADD`, the blacklist's `SET`, every
// `PUBLISH` — comes back `READONLY`. That is a Redis error rather than a
// connection error, so the default (`null`) keeps the client happily pinned to
// the wrong node until something unrelated drops the socket.
//
// Matched narrowly and by nothing else: reconnecting on ordinary errors turns
// one bad command into connection thrash. `true` rather than `2` — reconnect
// but do not resend — because every caller here already degrades on a single
// failed command, and a resend is the one behaviour none of them asked for.
function reconnectOnReadOnly(err: Error): boolean {
  return err.message.includes("READONLY");
}

// Shared so a change to either is a change to both. The TLS derivation was
// written out twice, and a connection that cannot verify its server costs
// nothing visible: every replica falls back to its own memory, and every
// ceiling is silently multiplied by the replica count.
function connectionOptions(url: URL): RedisOptions {
  const ca = env.REDIS_CA_CERT;
  return {
    ...(url.protocol === "rediss:" && ca
      ? { tls: { ca: [ca], servername: url.hostname } }
      : {}),
    keepAlive: KEEPALIVE_MS,
  };
}

export function getRedis(): Redis | null {
  if (constructed) return client;
  constructed = true;
  try {
    const url = new URL(env.REDIS_URL);

    client = new Redis(env.REDIS_URL, {
      ...connectionOptions(url),
      commandTimeout:       COMMAND_TIMEOUT_MS,
      reconnectOnError:     reconnectOnReadOnly,
      maxRetriesPerRequest: 1,
      enableOfflineQueue:   false,
      lazyConnect:          false,
    });
    client.on("error", (err: unknown) => {
      logger.error({ err }, "redis: client error");
    });
    // The transitions, once each. A client that never reaches `ready` — a
    // `rediss://` server whose certificate REDIS_CA_CERT does not verify, say
    // — leaves the limiter, the blacklist and the event bus falling back per
    // instance without a word of their own, so this is where an operator
    // learns the shared state is not shared.
    client.on("ready", () => {
      logger.info({ host: url.hostname }, "redis: ready");
    });
    client.on("close", () => {
      logger.warn({ host: url.hostname }, "redis: connection closed");
    });
  } catch (err: unknown) {
    logger.error({ err }, "redis: failed to construct client");
    client = null;
  }
  return client;
}

export function createRedisSubscriber(): Redis | null {
  try {
    const url = new URL(env.REDIS_URL);

    // No `commandTimeout` here, and that is the difference between the two
    // connections: this one's job is to wait. A subscriber that has heard
    // nothing for an hour is working correctly, so the liveness question is
    // answered by `keepAlive` above rather than by a deadline on a reply.
    const subscriber = new Redis(env.REDIS_URL, {
      ...connectionOptions(url),
      maxRetriesPerRequest: null,
      enableOfflineQueue: true,
      lazyConnect: false,
      enableReadyCheck: false,
      autoResubscribe: false,
    });
    subscriber.on("error", (err: unknown) => {
      logger.error({ err }, "redis: subscriber error");
    });
    // A subscriber that drops is the one failure an SSE stream cannot see:
    // the stream stays open and simply hears nothing. Same transitions as
    // the shared client, so the two can be read side by side.
    subscriber.on("ready", () => {
      logger.info({ host: url.hostname }, "redis: subscriber ready");
    });
    subscriber.on("close", () => {
      logger.warn({ host: url.hostname }, "redis: subscriber connection closed");
    });
    return subscriber;
  } catch (err: unknown) {
    logger.error({ err }, "redis: failed to create subscriber");
    return null;
  }
}
