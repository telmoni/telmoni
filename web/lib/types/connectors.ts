import { z } from "zod";

import { AttemptOutcome, AttemptTrigger, DeliveryStatus, NotificationKind } from "@/lib/types/enums";

// A connection's delivery log as the service answers it, parsed on the server
// by its entity (`lib/server/entities/connectors.ts`) and in the browser by
// the log's dialog.

export const DeliveryAttemptSchema = z.object({
  id: z.string(),
  trigger: z.enum(AttemptTrigger),
  outcome: z.enum(AttemptOutcome),
  status_code: z.number().int().nullable(),
  duration_ms: z.number(),
  error: z.string().nullable(),
  created_at: z.string(),
});
export type DeliveryAttempt = z.infer<typeof DeliveryAttemptSchema>;

// One notice as it was queued for one connection. `pending` covers both the
// first try not yet made and a retry waiting on `next_attempt_at`.
export const DeliveryLogEntrySchema = z.object({
  id: z.string(),
  kind: z.enum(NotificationKind),
  subject: z.string(),
  body: z.string(),
  status: z.enum(DeliveryStatus),
  next_attempt_at: z.string(),
  last_error: z.string().nullable(),
  created_at: z.string(),
  updated_at: z.string(),
  attempts: z.array(DeliveryAttemptSchema),
});
export type DeliveryLogEntry = z.infer<typeof DeliveryLogEntrySchema>;

// Newest first; `next_before` is the cursor for the next, older page.
export const DeliveryLogPageSchema = z.object({
  deliveries: z.array(DeliveryLogEntrySchema),
  next_before: z.string().nullable(),
});
export type DeliveryLogPage = z.infer<typeof DeliveryLogPageSchema>;
