import { cache } from "react";

import { env } from "@/lib/env";
import { getRedis } from "@/lib/redis";
import { AnnouncementSchema, type ProjectNotification } from "@/lib/types/announcement";

export const REDIS_ANNOUNCEMENT_KEY = "telmoni:announcement";

/** One source's announcement: JSON as the schema has it, or plain text where
 *  it is none; `null` for nothing, or a shape that is no announcement. */
function announcementOf(raw: string | null | undefined): ProjectNotification | null {
  const text = raw?.trim();
  if (!text) return null;
  let body: unknown;
  try {
    body = JSON.parse(text);
  } catch {
    return { message: text };
  }
  const parsed = AnnouncementSchema.safeParse(body);
  return parsed.success ? parsed.data : null;
}

/** The banner's announcement: the one broadcast through Redis, else the one
 *  the environment holds, else none. */
export const fetchProjectAnnouncement = cache(async (): Promise<ProjectNotification | null> => {
  let broadcast: string | null = null;
  try {
    broadcast = (await getRedis()?.get(REDIS_ANNOUNCEMENT_KEY)) ?? null;
  } catch {
    // Redis down or slow: the client already logged it, and the banner is
    // the one read in the console that may quietly fall back — to the
    // environment, or to nothing.
  }
  return announcementOf(broadcast) ?? announcementOf(env.TELMONI_ANNOUNCEMENT);
});
