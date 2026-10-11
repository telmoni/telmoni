import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

import { identityContext, projectHeaders } from "./identity-context";

// The field inventory: every attribute ingest reads, the spans it is read on,
// what becomes of it, the content mode it needs, and its rule. The server is
// the one source, so a row it adds needs no change here; `on` and `needs` are
// its words, shown as they come.
const InventoryRowSchema = z.object({
  attribute: z.string(),
  on: z.string(),
  kept: z.string(),
  needs: z.string(),
  rule: z.string(),
});

const FieldInventorySchema = z.object({
  conventions_commit: z.string(),
  rows: z.array(InventoryRowSchema),
});
export type FieldInventory = z.infer<typeof FieldInventorySchema>;

// `null` is an outage, or an answer this console cannot read: every role on
// the project reads the inventory, so no refusal is expected here.
export const fetchFieldInventory = cache(
  async (projectId: string): Promise<FieldInventory | null> => {
    if (!projectId) return null;
    const ctx = await identityContext();
    if (!ctx) return null;
    try {
      const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/telemetry/field-inventory`, {
        headers: projectHeaders(ctx, projectId),
        cache: "no-store",
      });
      if (!res.ok) {
        logger.warn(
          { fetcher: "fetchFieldInventory", status: res.status },
          "entities: upstream refused",
        );
        return null;
      }
      const parsed = FieldInventorySchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchFieldInventory", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return null;
      }
      return parsed.data;
    } catch (err) {
      logger.warn(
        {
          fetcher: "fetchFieldInventory",
          error: err instanceof Error ? err.message : String(err),
        },
        "entities: fetch failed",
      );
      return null;
    }
  },
);
