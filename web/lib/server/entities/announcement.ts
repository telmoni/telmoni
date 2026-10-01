import { cache } from "react";
import { getRedis } from "@/lib/redis";
import type { ProjectNotification } from "@/lib/types/announcement";

export const REDIS_ANNOUNCEMENT_KEY = "telmoni:announcement";

export const fetchProjectAnnouncement = cache(
  async (): Promise<ProjectNotification | null> => {
    try {
      const redis = getRedis();
      if (redis) {
        const raw = await redis.get(REDIS_ANNOUNCEMENT_KEY);
        if (raw && raw.trim().length > 0) {
          try {
            const parsed = JSON.parse(raw);
            if (typeof parsed === "string") {
              return { message: parsed };
            }
            if (parsed && typeof parsed.message === "string") {
              return {
                id: parsed.id,
                message: parsed.message,
                linkText: parsed.linkText,
                href: parsed.href,
              };
            }
          } catch {
            return { message: raw.trim() };
          }
        }
      }
    } catch {
      // Redis down or slow: the client already logged it, and the banner is
      // the one read in the console that may quietly fall back — to the
      // environment below, or to nothing.
    }

    const envRaw =
      process.env.TELMONI_ANNOUNCEMENT ||
      process.env.NEXT_PUBLIC_TELMONI_ANNOUNCEMENT;
    if (envRaw && envRaw.trim().length > 0) {
      try {
        const parsed = JSON.parse(envRaw);
        if (typeof parsed === "string") {
          return { message: parsed };
        }
        if (parsed && typeof parsed.message === "string") {
          return {
            id: parsed.id,
            message: parsed.message,
            linkText: parsed.linkText,
            href: parsed.href,
          };
        }
      } catch {
        return { message: envRaw.trim() };
      }
    }

    return null;
  },
);
