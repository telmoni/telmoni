import { cache } from "react";
import { z } from "zod";

import { fetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { logger } from "@/lib/logger";

import { IncidentSchema } from "./alerts";
import { identityContext, organizationHeaders } from "./identity-context";

// An open incident as the organization's Overview lists it, with the project
// it is on: its slug for the link to it under its alert, its name for the row.
const HealthIncidentSchema = IncidentSchema.extend({
  project: z.object({ id: z.string(), slug: z.string(), name: z.string() }),
});
export type HealthIncident = z.infer<typeof HealthIncidentSchema>;

export type OrganizationHealth =
  | { kind: "ok"; incidents: HealthIncident[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** The active organization's open incidents, acknowledged or not, on the
 *  projects the person can open, the newest first: none when all is well. */
export const fetchOrganizationHealth = cache(async (): Promise<OrganizationHealth> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  try {
    const res = await fetchWithTimeout(
      `${env.SERVER_URL}/internal/v1/telemetry/organizations/${encodeURIComponent(
        ctx.organizationId,
      )}/health`,
      { headers: organizationHeaders(ctx), cache: "no-store" },
    );
    if (res.status === 403) return { kind: "forbidden" };
    if (!res.ok) {
      logger.warn(
        { fetcher: "fetchOrganizationHealth", status: res.status },
        "entities: upstream refused",
      );
      return { kind: "unavailable" };
    }
    const parsed = z
      .object({ incidents: z.array(HealthIncidentSchema) })
      .safeParse(await res.json());
    if (!parsed.success) {
      logger.warn(
        { fetcher: "fetchOrganizationHealth", issues: parsed.error.issues },
        "entities: upstream shape mismatch",
      );
      return { kind: "unavailable" };
    }
    return { kind: "ok", incidents: parsed.data.incidents };
  } catch (err) {
    logger.warn(
      {
        fetcher: "fetchOrganizationHealth",
        error: err instanceof Error ? err.message : String(err),
      },
      "entities: fetch failed",
    );
    return { kind: "unavailable" };
  }
});
