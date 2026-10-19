import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

import { identityContext, organizationHeaders, projectHeaders } from "./identity-context";
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

// ⚠ **The organization's feed and a project's are DIFFERENT SETS**, not one
// filtered: the service picks its scope from the presence of `x-project-id`.
// Without a project it answers the rows that name no project, to the
// organization's owner and admins alone — the only way to read an
// `organization_alert`. With one it answers that project's rows, at any
// role on it — the only way to read a notice raised on a project.
export const fetchNotifications = cache(async (): Promise<NotificationFeed | null> => {
  const ctx = await identityContext();
  if (!ctx) return null;
  return read("fetchNotificationFeed", organizationHeaders(ctx));
});

export const fetchProjectNotifications = cache(
  async (projectId: string): Promise<NotificationFeed | null> => {
    // Without an id the request would name no project, and the service would
    // answer the organization's feed instead.
    if (!projectId) return null;
    const [ctx, project] = await Promise.all([identityContext(), fetchProject(projectId)]);
    // Nobody off the project is asked for it: the service would refuse them.
    if (!ctx || !project) return null;
    return read("fetchProjectNotifications", projectHeaders(ctx, projectId));
  },
);

async function read(
  fetcher: string,
  headers: Record<string, string>,
): Promise<NotificationFeed | null> {
  try {
    const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/v1/notifications/feed`, {
      headers,
      cache: "no-store",
    });
    if (!res.ok) {
      logger.warn({ fetcher, status: res.status }, "entities: upstream refused");
      return null;
    }
    const parsed = FeedSchema.safeParse(await res.json());
    if (!parsed.success) {
      logger.warn({ fetcher, issues: parsed.error.issues }, "entities: upstream shape mismatch");
      return null;
    }
    return parsed.data;
  } catch {
    logger.warn({ fetcher }, "entities: upstream error");
    return null;
  }
}
