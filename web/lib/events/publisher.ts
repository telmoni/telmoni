import { createHmac } from "node:crypto";

import { env } from "@/lib/env";
import { getRedis } from "@/lib/redis";
import type { RealtimeEvent } from "./types";
import { logger } from "@/lib/logger";

// Domain separation, so a value keyed for a channel name can never collide
// with anything else this secret is ever asked to key. Versioned because
// changing the derivation renames every channel, and the name is the thing
// publisher and subscriber have to agree on.
const USER_CHANNEL_LABEL = "telmoni:events:user:v1";

// ⚠ **The address never reaches Redis, because `PUBSUB CHANNELS` would hand
// the whole customer list to anyone who ran it.** The store is private, authed
// and TLS'd, so this is defence in depth rather than a hole — but a channel
// name is not a place that needed an email address in it, and one command
// turned the event bus into a directory.
//
// **HMAC and not a bare digest.** SHA-256 of an address is confirmable
// offline: take a breach list, hash every entry, look for a match. A key the
// reader does not hold makes the same list worthless.
//
// **`AUTH_SECRET` and not a secret of its own.** It is already deployed and
// already rotated as a unit, and the coupling is the one you want: rotating it
// invalidates every sealed session, so every subscriber is forced to reconnect
// and re-derive at the same moment the publishers do. A separate secret would
// need that coordination built by hand for no extra strength.
//
// 128 bits is far past collision range for a channel namespace, and a full
// digest would just make the name harder to read in an incident.
export function userChannel(email: string): string {
  const fingerprint = createHmac("sha256", env.AUTH_SECRET)
    .update(`${USER_CHANNEL_LABEL}\n${email.trim().toLowerCase()}`)
    .digest("hex")
    .slice(0, 32);
  return `bfev:user:${fingerprint}`;
}

// ⚠ **Not keyed, deliberately.** An organization id is a minted `org_…` value
// that names nobody, so there is nothing here for a keyed digest to hide —
// and hiding it would cost the one thing a readable channel name is worth:
// being able to see which tenant a stuck subscriber belongs to.
export function organizationChannel(organizationId: string): string {
  return `bfev:organization:${organizationId.trim()}`;
}

export async function publishEvent(
  channel: string,
  event: RealtimeEvent,
): Promise<boolean> {
  const redis = getRedis();
  if (!redis) return false;

  try {
    await redis.publish(channel, JSON.stringify(event));
    return true;
  } catch (err) {
    logger.warn({ channel, err }, "events: failed to publish");
    return false;
  }
}

export async function publishToAll(
  channels: readonly (string | null | undefined)[],
  event: RealtimeEvent,
): Promise<void> {
  const unique = new Set(channels.filter((c): c is string => typeof c === "string" && c.length > 0));
  for (const channel of unique) {
    await publishEvent(channel, event);
  }
}
