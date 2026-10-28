import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";

import { IncidentSchema } from "./alerts";
import { identityContext, organizationHeaders } from "./identity-context";
import { readJson } from "./read";

// An open incident as the organization's Overview lists it, with the project
// it is on: its slug for the link to it under its alert, its name for the row.
const HealthIncidentSchema = IncidentSchema.extend({
  project: z.object({ id: z.string(), slug: z.string(), name: z.string() }),
});
export type HealthIncident = z.infer<typeof HealthIncidentSchema>;

export const OrganizationHealthSchema = z.object({ incidents: z.array(HealthIncidentSchema) });

export type OrganizationHealth =
  | { kind: "ok"; incidents: HealthIncident[] }
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** The active organization's open incidents, acknowledged or not, on the
 *  projects the person can open, the newest first: none when all is well. */
export const fetchOrganizationHealth = cache(async (): Promise<OrganizationHealth> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const read = await readJson(
    "fetchOrganizationHealth",
    `${env.SERVER_URL}/internal/v1/telemetry/organizations/${encodeURIComponent(
      ctx.organizationId,
    )}/health`,
    { headers: organizationHeaders(ctx), cache: "no-store" },
    OrganizationHealthSchema,
  );
  return read.kind === "ok" ? { kind: "ok", incidents: read.value.incidents } : read;
});
