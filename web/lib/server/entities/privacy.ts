import { cache } from "react";
import { z } from "zod";

import { valueOf } from "./read";
import { readLane } from "./traces";

// A project's privacy posture: what its spans arrived with over the server's
// window, by the field inventory's rows, and how long spans and the hourly
// totals that outlive them are kept. The server holds every number.
export const PrivacyPostureSchema = z.object({
  days: z.number().int(),
  since: z.string(),
  spans: z.number().int(),
  sent: z.array(z.object({ attribute: z.string(), spans: z.number().int() })),
  retention: z.object({
    span_days: z.number().int(),
    rollup_months: z.number().int(),
  }),
});
export type PrivacyPosture = z.infer<typeof PrivacyPostureSchema>;

// `null` is an outage, or an answer this console cannot read: every role on
// the project reads it, so no refusal is expected here.
export const fetchPrivacyPosture = cache(
  async (projectId: string): Promise<PrivacyPosture | null> => {
    return valueOf(
      await readLane("fetchPrivacyPosture", projectId, "/privacy", PrivacyPostureSchema),
    );
  },
);
