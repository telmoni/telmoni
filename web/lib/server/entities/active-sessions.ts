import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

import { accountHeaders } from "./identity-context";

// A sign-in of the person's own, as Account › Privacy lists it — not a
// session of a project's runs, which `sessions.ts` reads.
const ActiveSessionSchema = z.object({
  id: z.string(),
  user_agent: z.string().nullable(),
  created_at: z.string(),
  last_seen_at: z.string(),
});
export type ActiveSession = z.infer<typeof ActiveSessionSchema>;

export const fetchActiveSessions = cache(
  async (): Promise<ActiveSession[] | null> => {
    // The person's own sessions, whichever organization the console is in.
    const headers = await accountHeaders();
    if (!headers) return null;
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/v1/auth/sessions`,
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
        .object({ sessions: z.array(ActiveSessionSchema) })
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
