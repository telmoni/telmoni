import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

import { identityContext, organizationHeaders } from "./identity-context";

const FeedItemSchema = z.object({
  id: z.string(),
  kind: z.string(),
  title: z.string(),
  body: z.string(),
  read_at: z.string().nullable(),
  created_at: z.string(),
});
export type FeedItem = z.infer<typeof FeedItemSchema>;

const FeedSchema = z.object({
  items: z.array(FeedItemSchema),
  unread: z.number(),
});
export type NotificationFeed = z.infer<typeof FeedSchema>;

// The organization's own feed: the rows that name no project, answered to its
// owner and admins alone — the only way to read an `organization_alert`.
// ⚠ **A project's feed is a DIFFERENT SET**, not a filtered view of this one:
// the service picks its scope from the presence of `x-project-id`, and with a
// project it answers that project's rows — `member_added`, the `connector_*`
// kinds. Their reader, `fetchProjectNotifications`, left with the project
// Overview's activity section (2026-10-08) and is in git for its rebuild.
export const fetchNotifications = cache(
  async (): Promise<NotificationFeed | null> => read(),
);

async function read(): Promise<NotificationFeed | null> {
  {
    const ctx = await identityContext();
    if (!ctx) return null;
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/notifications/feed`,
        {
          headers: organizationHeaders(ctx),
          cache: "no-store",
        },
      );
      if (!res.ok) {
        logger.warn(
          { fetcher: "fetchNotificationFeed", status: res.status },
          "entities: upstream refused",
        );
        return null;
      }
      const parsed = FeedSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchNotificationFeed", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return null;
      }
      return parsed.data;
    } catch {
      logger.warn(
        { fetcher: "fetchNotificationFeed" },
        "entities: upstream error",
      );
      return null;
    }
  }
}
