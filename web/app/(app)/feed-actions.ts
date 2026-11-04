"use server";

import { z } from "zod";

import { administers } from "@/lib/roles";
import { identityContext } from "@/lib/server/entities/identity-context";
import {
  fetchNotifications,
  fetchProjectNotifications,
  type NotificationFeed,
} from "@/lib/server/entities/notification-feed";

// A feed of the place the console stands in, as the bell reads it: a
// project's, any member's, or the organization's own, its owners' and admins'
// alone. A Server Action takes whatever is posted to it, so the
// scope is checked, and an organization that is not the one the session
// stands in reads nothing rather than another's feed.

const Scope = z.discriminatedUnion("kind", [
  z.strictObject({ kind: z.literal("project"), projectId: z.string().min(1).max(64) }),
  z.strictObject({ kind: z.literal("organization"), organizationId: z.string().min(1).max(64) }),
]);
export type FeedScope = z.infer<typeof Scope>;

/** The scope's feed, newest first with its unread count; none where it
 *  cannot be read or is not the person's to read. */
export async function readFeedAction(scope: unknown): Promise<NotificationFeed | null> {
  const asked = Scope.safeParse(scope);
  if (!asked.success) return null;
  if (asked.data.kind === "project") return fetchProjectNotifications(asked.data.projectId);
  const ctx = await identityContext();
  if (!ctx || ctx.organizationId !== asked.data.organizationId || !administers(ctx.role)) return null;
  return fetchNotifications();
}
