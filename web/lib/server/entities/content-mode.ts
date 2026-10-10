import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { ContentMode } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";

// A project's content mode, and the modes its switch offers. The server
// decides both, so a mode it starts offering needs no change here.
const ContentModeSettingSchema = z.object({
  content_mode: z.enum(ContentMode),
  offered: z.array(z.enum(ContentMode)),
});
export type ContentModeSetting = z.infer<typeof ContentModeSettingSchema>;

// `null` is an outage, or an answer this console cannot read: every role on
// the project reads its mode, so no refusal is expected here.
export const fetchContentMode = cache(
  async (projectId: string): Promise<ContentModeSetting | null> => {
    if (!projectId) return null;
    const ctx = await identityContext();
    if (!ctx) return null;
    try {
      const res = await fetchWithTimeout(`${env.SERVER_URL}/internal/telemetry/content-mode`, {
        headers: projectHeaders(ctx, projectId),
        cache: "no-store",
      });
      if (!res.ok) {
        logger.warn(
          { fetcher: "fetchContentMode", status: res.status },
          "entities: upstream refused",
        );
        return null;
      }
      const parsed = ContentModeSettingSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchContentMode", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return null;
      }
      return parsed.data;
    } catch (err) {
      logger.warn(
        {
          fetcher: "fetchContentMode",
          error: err instanceof Error ? err.message : String(err),
        },
        "entities: fetch failed",
      );
      return null;
    }
  },
);
