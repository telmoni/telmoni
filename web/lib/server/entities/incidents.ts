import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";
import { IncidentStatus, Severity, Verdict } from "@/lib/types/enums";

import { identityContext, projectHeaders } from "./identity-context";

// One of the project's incidents with the votes on it and the person's own,
// as the page every notice of it links to reads it.
const IncidentVotesSchema = z.object({
  id: z.string(),
  rule_name: z.string(),
  status: z.enum(IncidentStatus),
  severity: z.enum(Severity),
  opened_at: z.string(),
  resolved_at: z.string().nullable(),
  useful: z.number().int(),
  noise: z.number().int(),
  yours: z.enum(Verdict).nullable(),
});
export type IncidentVotes = z.infer<typeof IncidentVotesSchema>;

export type IncidentAccess =
  | { kind: "ok"; incident: IncidentVotes }
  /** No incident with that id in this project. */
  | { kind: "missing" }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** One incident, by its id. An id the path cannot be — not a UUID — is read
 *  as missing, as the server would answer one that is not the project's. */
export const fetchIncident = cache(
  async (projectId: string, incidentId: string): Promise<IncidentAccess> => {
    if (!projectId) return { kind: "unavailable" };
    const ctx = await identityContext();
    if (!ctx) return { kind: "unavailable" };
    try {
      const res = await fetchWithTimeout(
        `${env.SERVER_URL}/internal/v1/telemetry/incidents/${encodeURIComponent(incidentId)}`,
        { headers: projectHeaders(ctx, projectId), cache: "no-store" },
      );
      if (res.status === 403) return { kind: "forbidden" };
      if (res.status === 404 || res.status === 400) return { kind: "missing" };
      if (!res.ok) {
        logger.warn({ fetcher: "fetchIncident", status: res.status }, "entities: upstream refused");
        return { kind: "unavailable" };
      }
      const parsed = IncidentVotesSchema.safeParse(await res.json());
      if (!parsed.success) {
        logger.warn(
          { fetcher: "fetchIncident", issues: parsed.error.issues },
          "entities: upstream shape mismatch",
        );
        return { kind: "unavailable" };
      }
      return { kind: "ok", incident: parsed.data };
    } catch (err) {
      logger.warn(
        {
          fetcher: "fetchIncident",
          error: err instanceof Error ? err.message : String(err),
        },
        "entities: fetch failed",
      );
      return { kind: "unavailable" };
    }
  },
);
