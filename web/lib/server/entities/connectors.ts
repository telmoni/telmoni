import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { NotificationKind } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { fetchProject } from "./projects";

const NotificationKindSchema = z.enum(NotificationKind);

// A connection as the service lists it: the workspace and the channel a
// person recognises a row by — or, for a webhook, its host — and its state.
// Never the target URL — the service seals it and no read returns it — and
// never a webhook's signing secret, which is answered once at creation.
export const ConnectionSchema = z.object({
  id: z.string(),
  provider: z.enum(["slack", "discord", "webhook"]),
  external_workspace_id: z.string(),
  external_workspace_name: z.string().nullable(),
  channel_id: z.string(),
  channel_name: z.string(),
  status: z.enum(["active", "errored", "revoked"]),
  last_error: z.string().nullable(),
  last_delivery_at: z.string().nullable(),
  created_at: z.string(),
  // `null` is every kind, the kinds a future release adds included; only a
  // webhook ever narrows it.
  event_kinds: z.array(NotificationKindSchema).nullable(),
  // Set only while a rotated webhook still signs with its old secret too.
  previous_secret_expires_at: z.string().nullable(),
});
export type Connection = z.infer<typeof ConnectionSchema>;

const DeliveryAttemptSchema = z.object({
  id: z.string(),
  trigger: z.enum(["scheduled", "manual"]),
  outcome: z.enum(["delivered", "failed"]),
  status_code: z.number().int().nullable(),
  duration_ms: z.number(),
  error: z.string().nullable(),
  created_at: z.string(),
});
export type DeliveryAttempt = z.infer<typeof DeliveryAttemptSchema>;

// One notice as it was queued for one connection. `pending` covers both the
// first try not yet made and a retry waiting on `next_attempt_at`.
const DeliveryLogEntrySchema = z.object({
  id: z.string(),
  kind: NotificationKindSchema,
  subject: z.string(),
  body: z.string(),
  status: z.enum(["pending", "delivered", "failed"]),
  next_attempt_at: z.string(),
  last_error: z.string().nullable(),
  created_at: z.string(),
  updated_at: z.string(),
  attempts: z.array(DeliveryAttemptSchema),
});
export type DeliveryLogEntry = z.infer<typeof DeliveryLogEntrySchema>;

// Newest first; `next_before` is the cursor for the next, older page.
export const DeliveryLogPageSchema = z.object({
  deliveries: z.array(DeliveryLogEntrySchema),
  next_before: z.string().nullable(),
});
export type DeliveryLogPage = z.infer<typeof DeliveryLogPageSchema>;

const ConnectorListingSchema = z.object({
  // Which connectors this deployment can connect: the vendors it holds
  // credentials for, and the webhook whenever it holds a key. A tile for one
  // it cannot renders as unavailable.
  enabled: z.object({ slack: z.boolean(), discord: z.boolean(), webhook: z.boolean() }),
  connections: z.array(ConnectionSchema),
});
export type ConnectorListing = z.infer<typeof ConnectorListingSchema>;

// `null` is the outage posture and the not-configured posture alike: the page
// renders the tiles as it did before the lane existed.
export const fetchConnectors = cache(
  async (projectId: string): Promise<ConnectorListing | null> => {
    const base = env.SERVER_URL;
    if (!projectId) return null;
    const [ctx, project] = await Promise.all([identityContext(), fetchProject(projectId)]);
    if (!ctx || !project) return null;
    try {
      const res = await fetchWithTimeout(`${base}/internal/connectors`, {
        headers: projectHeaders(ctx, projectId),
        cache: "no-store",
      });
      if (!res.ok) {
        logger.warn(
          { fetcher: "fetchConnectors", status: res.status },
          "entities: upstream refused",
        );
        return null;
      }
      const parsed = ConnectorListingSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchConnectors", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return null;
      }
      return parsed.data;
    } catch {
      logger.warn({ fetcher: "fetchConnectors" }, "entities: upstream error");
      return null;
    }
  },
);
