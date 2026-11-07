import { cache } from "react";
import { z } from "zod";

import { env } from "@/lib/env";

import { identityContext, organizationHeaders } from "./identity-context";
import { readJson } from "./read";

// One project's caps on its organization's pool; `null` shares it whole.
export const CapsSchema = z.object({
  spans: z.number().int().nullable(),
  content_bytes: z.number().int().nullable(),
  alerts: z.number().int().nullable(),
  monitors: z.number().int().nullable(),
  objectives: z.number().int().nullable(),
  alert_deliveries_per_day: z.number().int().nullable(),
});
export type Caps = z.infer<typeof CapsSchema>;

// How an organization divides its pool among its projects: what its plan
// holds of each limit (`null` where nothing is sold, or the plan holds no
// number of it), and each project the reader can open with its caps and what
// it has used — its spans this month `null` where the span store did not
// answer, and its deliveries today read only where capped.
export const LimitsSchema = z.object({
  plan: z.object({
    spans: z.number().int().nullable(),
    content_bytes: z.number().int().nullable(),
    alerts: z.number().int().nullable(),
    alert_deliveries_per_day: z.number().int().nullable(),
  }),
  projects: z.array(
    z.object({
      id: z.string(),
      name: z.string(),
      caps: CapsSchema,
      used: z.object({
        spans: z.number().int().nullable(),
        content_bytes: z.number().int(),
        alerts: z.number().int(),
        monitors: z.number().int(),
        objectives: z.number().int(),
        alert_deliveries_today: z.number().int().nullable(),
      }),
    }),
  ),
});
export type Limits = z.infer<typeof LimitsSchema>;

// `null` is an outage, or an answer this console cannot read: every member
// reads it, so no refusal is expected here.
export const fetchLimits = cache(async (): Promise<Limits | null> => {
  const ctx = await identityContext();
  if (!ctx) return null;
  const read = await readJson(
    "fetchLimits",
    `${env.SERVER_URL}/internal/v1/telemetry/limits`,
    { headers: organizationHeaders(ctx) },
    LimitsSchema,
  );
  return read.kind === "ok" ? read.value : null;
});
