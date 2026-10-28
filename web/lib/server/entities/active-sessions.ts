import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";

import { accountHeaders } from "./identity-context";
import { readJson } from "./read";

// A sign-in of the person's own, as Account › Privacy lists it — not a
// session of a project's runs, which `sessions.ts` reads.
const ActiveSessionSchema = z.object({
  id: z.string(),
  user_agent: z.string().nullable(),
  created_at: z.string(),
  last_seen_at: z.string(),
});
export type ActiveSession = z.infer<typeof ActiveSessionSchema>;

export const ActiveSessionListSchema = z.object({ sessions: z.array(ActiveSessionSchema) });

export const fetchActiveSessions = cache(
  async (): Promise<ActiveSession[] | null> => {
    // The person's own sessions, whichever organization the console is in.
    const headers = await accountHeaders();
    if (!headers) return null;
    const read = await readJson(
      "fetchActiveSessions",
      `${env.SERVER_URL}/internal/v1/auth/sessions`,
      { headers },
      ActiveSessionListSchema,
    );
    return read.kind === "ok" ? read.value.sessions : null;
  },
);
