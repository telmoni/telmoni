import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";

import { identityContext, organizationHeaders } from "./identity-context";
import { readJson } from "./read";

// One project's spend this period, against its budget where it set one: a
// cost alert summed over the month with no filter.
const ProjectSpendSchema = z.object({
  project: z.object({ id: z.string(), slug: z.string(), name: z.string() }),
  spent: z.number(),
  unpriced_calls: z.number(),
  budget: z.number().nullable(),
});
export type ProjectSpend = z.infer<typeof ProjectSpendSchema>;

export const OrganizationSpendSchema = z.object({
  period_start: z.string(),
  period_end: z.string(),
  total: z.number(),
  projects: z.array(ProjectSpendSchema),
});

export type OrganizationSpend =
  | ({ kind: "ok" } & z.infer<typeof OrganizationSpendSchema>)
  | { kind: "forbidden" }
  | { kind: "unavailable" };

/** What each of the active organization's projects spent this period, the
 *  costliest first, a project with no model call left out: its owners' and
 *  admins' alone. */
export const fetchOrganizationSpend = cache(async (): Promise<OrganizationSpend> => {
  const ctx = await identityContext();
  if (!ctx) return { kind: "unavailable" };
  const read = await readJson(
    "fetchOrganizationSpend",
    `${env.SERVER_URL}/internal/v1/telemetry/organizations/${encodeURIComponent(
      ctx.organizationId,
    )}/spend`,
    { headers: organizationHeaders(ctx), cache: "no-store" },
    OrganizationSpendSchema,
  );
  return read.kind === "ok" ? { kind: "ok", ...read.value } : read;
});
