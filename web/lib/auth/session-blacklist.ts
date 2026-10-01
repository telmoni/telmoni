
import { getRedis } from "@/lib/redis";
import { logger } from "@/lib/logger";

const BLACKLIST_PREFIX = "bfsb:";
const DEFAULT_BLACKLIST_TTL_SECONDS = 60 * 60 * 24;

// Pub/sub channel that fires the moment a session is blacklisted, so an open
// SSE stream on ANY instance can close at once instead of on its next 25 s
// poll of the key. The key is still written first: the channel reaches only
// whoever is listening at that instant, the key reaches every check after.
export function sessionRevokedChannel(sessionId: string): string {
  return `bfev:session:${sessionId}`;
}

const memoryBlacklist = new Map<string, number>();

// ⚠ **An entry leaves this map only when somebody asks about that exact
// session again, and after a revocation nobody ever does.** The whole point of
// blacklisting is that the session stops being presented, so its entry is
// reached once on the next request and then never again — it sits there for
// the life of the instance. The sliding-window limiter next door sweeps its
// own map for this reason; this one was the same shape with no sweep.
//
// Purely a memory fix: an expired entry already answers `false`, so removing
// it early changes nothing anyone can observe.
const SWEEP_INTERVAL_MS = 60_000;
let lastSweep = 0;

function sweepIfStale(now: number) {
  if (now - lastSweep < SWEEP_INTERVAL_MS) return;
  lastSweep = now;
  for (const [id, expiresAt] of memoryBlacklist) {
    if (now >= expiresAt) memoryBlacklist.delete(id);
  }
}

function safeGetRedis() {
  if (!process.env.REDIS_URL) return null;
  try {
    return getRedis();
  } catch {
    return null;
  }
}

export async function blacklistSession(
  sessionId: string | null,
  ttlSeconds: number = DEFAULT_BLACKLIST_TTL_SECONDS,
): Promise<void> {
  if (!sessionId) return;

  const now = Date.now();
  sweepIfStale(now);
  memoryBlacklist.set(sessionId, now + ttlSeconds * 1000);

  const redis = safeGetRedis();
  if (redis && redis.status === "ready") {
    try {
      await redis.set(`${BLACKLIST_PREFIX}${sessionId}`, "1", "EX", ttlSeconds);
      await redis.publish(sessionRevokedChannel(sessionId), "revoked");
    } catch (err) {
      logger.error({ err }, "session-blacklist: redis error on blacklistSession");
    }
  }
}

export async function isSessionBlacklisted(
  sessionId: string | null,
): Promise<boolean> {
  if (!sessionId) return false;

  const now = Date.now();
  sweepIfStale(now);

  const memExpiry = memoryBlacklist.get(sessionId);
  if (memExpiry !== undefined) {
    if (now < memExpiry) {
      return true;
    }
    memoryBlacklist.delete(sessionId);
  }

  const redis = safeGetRedis();
  if (redis && redis.status === "ready") {
    try {
      const val = await redis.get(`${BLACKLIST_PREFIX}${sessionId}`);
      return val !== null;
    } catch (err) {
      logger.error({ err }, "session-blacklist: redis error on isSessionBlacklisted");
      return false;
    }
  }

  return false;
}

export function clearBlacklistForTest(): void {
  memoryBlacklist.clear();
  lastSweep = 0;
}

export function blacklistSizeForTest(): number {
  return memoryBlacklist.size;
}
