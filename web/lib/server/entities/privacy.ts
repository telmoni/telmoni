import { cache } from "react";
import { z } from "zod";

import { valueOf } from "./read";
import { readLane } from "./traces";

// The server's project policy, shared by reads and writes.
export const DoorScrubSchema = z.object({ door_scrub_secrets: z.boolean(), door_scrub_pii: z.boolean() }).strict();
export type DoorScrub = z.infer<typeof DoorScrubSchema>;

// What spans carried in the server's window and how long they are kept.
export const PrivacyPostureSchema = z.object({
  door_scrub: DoorScrubSchema,
  days: z.number().int(),
  since: z.string(),
  spans: z.number().int(),
  sent: z.array(z.object({ attribute: z.string(), spans: z.number().int() })),
  retention: z.object({
    span_days: z.number().int(),
    max_span_days: z.number().int(),
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
