import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

import { accountHeaders } from "./identity-context";

const SessionSchema = z.object({
  id: z.string(),
  user_agent: z.string().nullable(),
  created_at: z.string(),
  last_seen_at: z.string(),
});
export type ActiveSession = z.infer<typeof SessionSchema>;

export const fetchActiveSessions = cache(
  async (): Promise<ActiveSession[] | null> => {
    // The person's own sessions, whichever organization the console is in.
    const headers = await accountHeaders();
    if (!headers) return null;
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/auth/sessions`,
        { headers },
      );
      if (!res.ok) {
        logger.warn(
          { fetcher: "fetchActiveSessions", status: res.status },
          "entities: upstream refused",
        );
        return null;
      }
      const parsed = z
        .object({ sessions: z.array(SessionSchema) })
        .safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchActiveSessions", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return null;
      }
      return parsed.data.sessions;
    } catch {
      logger.warn({ fetcher: "fetchActiveSessions" }, "entities: upstream error");
      return null;
    }
  },
);
