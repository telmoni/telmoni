import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";
import { NotificationKind } from "@/lib/types/enums";

import { identityContext, organizationHeaders, projectHeaders } from "./identity-context";
import { fetchProject } from "./projects";
import { readJson, valueOf } from "./read";

// What an incident's notice carries besides its words: its incident and its
// alert (`crates/telemetry/src/notify.rs`), the incident its row links to.
const IncidentMetadataSchema = z.object({ incident_id: z.guid(), rule_id: z.guid() });

const INCIDENT_KINDS: ReadonlySet<string> = new Set([
  NotificationKind.IncidentOpened,
  NotificationKind.IncidentResolved,
]);

// Only an incident's notice keeps its metadata, checked: one that names no
// incident stays on the feed, linking nowhere, rather than failing the feed.
const FeedItemSchema = z
  .object({
    id: z.string(),
    kind: z.string(),
    title: z.string(),
    body: z.string(),
    metadata: z.unknown(),
    read_at: z.string().nullable(),
    created_at: z.string(),
  })
  .transform(({ metadata, ...item }) => {
    const incident = INCIDENT_KINDS.has(item.kind) ? IncidentMetadataSchema.safeParse(metadata) : null;
    return { ...item, metadata: incident?.success ? incident.data : null };
  });
export type FeedItem = z.infer<typeof FeedItemSchema>;

export const FeedSchema = z.object({
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
  return valueOf(
    await readJson(
      fetcher,
      `${env.SERVER_URL}/internal/v1/notifications/feed`,
      { headers, cache: "no-store" },
      FeedSchema,
    ),
  );
}
