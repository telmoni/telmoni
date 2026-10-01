import type Redis from "ioredis";

import { createRedisSubscriber } from "@/lib/redis";
import { logger } from "@/lib/logger";

type Listener = (channel: string, message: string) => void;

let connection: Redis | null = null;
let constructed = false;
const listeners = new Map<string, Set<Listener>>();

// ⚠ **A SUBSCRIBE that fails on a live connection leaves that channel deaf,
// and nothing downstream can tell.** The stream stays open, the keepalive
// keeps ticking, and the events simply never arrive — the same silence as a
// channel nobody published to. `autoResubscribe` is off (the table here is the
// record, not ioredis's), so a reconnect re-sends everything.
const RESUBSCRIBE_DELAY_MS = 1_000;
let retryTimer: NodeJS.Timeout | null = null;

function subscriber(): Redis | null {
  if (constructed) return connection;
  constructed = true;
  connection = createRedisSubscriber();
  connection?.on("message", (channel: string, message: string) => {
    const set = listeners.get(channel);
    if (!set) return;
    for (const listener of set) {
      try {
        listener(channel, message);
      } catch (err) {
        logger.warn({ channel, err }, "events: listener failed");
      }
    }
  });
  connection?.on("ready", () => {
    send([...listeners.keys()]);
  });
  return connection;
}

function send(channels: readonly string[]) {
  if (channels.length === 0 || connection?.status !== "ready") return;
  connection.subscribe(...channels).catch((err: unknown) => {
    logger.warn({ err, channels }, "events: subscribe failed, retrying");
    scheduleRetry();
  });
}

// Re-sends the WHOLE table rather than the channels that failed. SUBSCRIBE on
// a channel already subscribed is a no-op that answers with the count, so the
// broad retry costs one command and cannot miss a channel that arrived between
// the failure and the tick. One timer at a time: several failures inside the
// same second are one outage, not several.
function scheduleRetry() {
  if (retryTimer) return;
  retryTimer = setTimeout(() => {
    retryTimer = null;
    const wanted = [...listeners.keys()];
    if (wanted.length === 0) return;
    if (connection?.status !== "ready") {
      // The connection went down after all, which the `ready` handler covers
      // on its own terms — it re-sends this same table. Nothing to do but stop
      // holding a timer open for it.
      return;
    }
    send(wanted);
  }, RESUBSCRIBE_DELAY_MS);
}

export function subscribe(channels: readonly string[], listener: Listener): () => void {
  const redis = subscriber();
  if (!redis) return () => {};

  const wanted = [...new Set(channels)];
  const fresh: string[] = [];
  for (const channel of wanted) {
    let set = listeners.get(channel);
    if (!set) {
      set = new Set();
      listeners.set(channel, set);
      fresh.push(channel);
    }
    set.add(listener);
  }
  send(fresh);

  let stopped = false;
  return () => {
    if (stopped) return;
    stopped = true;
    const idle: string[] = [];
    for (const channel of wanted) {
      const set = listeners.get(channel);
      if (!set) continue;
      set.delete(listener);
      if (set.size === 0) {
        listeners.delete(channel);
        idle.push(channel);
      }
    }
    if (idle.length > 0 && redis.status === "ready") {
      redis.unsubscribe(...idle).catch(() => {});
    }
  };
}
