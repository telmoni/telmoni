import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

import {
  identityContext,
  organizationHeaders,
  projectHeaders,
} from "./identity-context";
import { fetchProject } from "./projects";

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

export const fetchNotifications = cache(
  async (): Promise<NotificationFeed | null> => read(),
);

// ⚠ **The project's feed is a DIFFERENT SET from the organization's**, not a
// filtered view of it. The service picks its scope from the presence of
// `x-project-id`: with a project it answers that project's rows, without one it answers
// the rows that name no project and refuses anybody but the organization's owner.
// So this is the only way to read a `member_added` or a `connector_*` notice,
// and `fetchNotifications` is the only way to read an `organization_alert`.
export const fetchProjectNotifications = cache(
  async (projectId: string): Promise<NotificationFeed | null> => readProject(projectId),
);

async function readProject(projectId: string): Promise<NotificationFeed | null> {
  const [ctx, project] = await Promise.all([identityContext(), fetchProject(projectId)]);
  // No membership row means the service would refuse the project anyway.
  // Nothing to show is the right answer.
  if (!ctx || !project) return null;
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/notifications/feed`,
      {
        headers: projectHeaders(ctx, projectId),
        cache: "no-store",
      },
    );
    if (!res.ok) {
      logger.warn(
        { fetcher: "fetchProjectNotifications", status: res.status },
        "entities: upstream refused",
      );
      return null;
    }
    const parsed = FeedSchema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "fetchProjectNotifications", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return null;
    }
    return parsed.data;
  } catch {
    logger.warn(
      { fetcher: "fetchProjectNotifications" },
      "entities: upstream error",
    );
    return null;
  }
}

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
