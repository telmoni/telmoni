import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { identityContext, projectHeaders } from "./identity-context";

// The layout a project's sessions and traces open in: the saved layout
// `view_id` names while it stands, or the console's own default
// (`lib/layout.ts`).
export const SessionLayoutSettingSchema = z.object({
  view_id: z.string().nullable(),
});
export type SessionLayoutSetting = z.infer<typeof SessionLayoutSettingSchema>;

// `null` is an outage, or an answer this console cannot read: every role on
// the project reads its layout, so no refusal is expected here.
export const fetchSessionLayout = cache(
  async (projectId: string): Promise<SessionLayoutSetting | null> => {
    if (!projectId) return null;
    const ctx = await identityContext();
    if (!ctx) return null;
    try {
      const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/session-layout`, {
        headers: projectHeaders(ctx, projectId),
        cache: "no-store",
      });
      if (!res.ok) {
        logger.warn(
          { fetcher: "fetchSessionLayout", status: res.status },
          "entities: upstream refused",
        );
        return null;
      }
      const parsed = SessionLayoutSettingSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchSessionLayout", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return null;
      }
      return parsed.data;
    } catch (err) {
      logger.warn(
        {
          fetcher: "fetchSessionLayout",
          error: err instanceof Error ? err.message : String(err),
        },
        "entities: fetch failed",
      );
      return null;
    }
  },
);
