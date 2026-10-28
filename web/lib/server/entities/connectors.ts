import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";
import { ConnectionStatus, ConnectorProvider, NotificationKind } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";
import { fetchProject } from "./projects";
import { readJson, valueOf } from "./read";

const NotificationKindSchema = z.enum(NotificationKind);

// A connection as the service lists it: the workspace and the channel a
// person recognises a row by — or, for a webhook, its host — and its state.
// Never the target URL — the service seals it and no read returns it — and
// never a webhook's signing secret, which is answered once at creation.
export const ConnectionSchema = z.object({
  id: z.string(),
  provider: z.enum(ConnectorProvider),
  external_workspace_id: z.string(),
  external_workspace_name: z.string().nullable(),
  channel_id: z.string(),
  channel_name: z.string(),
  status: z.enum(ConnectionStatus),
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

export {
  DeliveryLogPageSchema,
  type DeliveryAttempt,
  type DeliveryLogEntry,
  type DeliveryLogPage,
} from "@/lib/types/connectors";

export const ConnectorListingSchema = z.object({
  // Which connectors this deployment can connect: the vendors it holds
  // credentials for, and the webhook whenever it holds a key. A tile for one
  // it cannot renders as unavailable.
  enabled: z.object({ slack: z.boolean(), webhook: z.boolean() }),
  connections: z.array(ConnectionSchema),
});
export type ConnectorListing = z.infer<typeof ConnectorListingSchema>;

/** A connection as a change to the kinds it receives answers it. */
export const ConnectionAnswerSchema = z.object({ connection: ConnectionSchema });

// What the service answers when it mints a signing secret: the row, and the
// secret this one time. The host is the one thing the page will show of the
// endpoint, so it rides along to title the dialog.
export const SigningSecretSchema = z.object({
  connection: z.object({ id: z.string(), channel_name: z.string() }),
  signing_secret: z.string().min(1),
});

/** What a send now answers, a test or a resend: the vendor's refusal, if any. */
export const SendAnswerSchema = z.object({
  delivered: z.boolean(),
  error: z.string().nullable(),
});

/** Where a connect sends the browser, and the state its callback is checked by. */
export const AuthorizeSchema = z.object({ url: z.url(), state: z.string().min(1) });

// `null` is the outage posture and the not-configured posture alike: the page
// renders the tiles as it did before the lane existed.
export const fetchConnectors = cache(
  async (projectId: string): Promise<ConnectorListing | null> => {
    if (!projectId) return null;
    const [ctx, project] = await Promise.all([identityContext(), fetchProject(projectId)]);
    if (!ctx || !project) return null;
    return valueOf(
      await readJson(
        "fetchConnectors",
        `${env.SERVER_URL}/internal/v1/connectors`,
        { headers: projectHeaders(ctx, projectId), cache: "no-store" },
        ConnectorListingSchema,
      ),
    );
  },
);
