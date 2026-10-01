import { z } from "zod";

import { IncomingInviteSchema } from "@/lib/types/incoming-invite";

const InviteRefSchema = z.object({ inviteId: z.string() });

/// An organization whose ownership offer or owner just changed — or, when a
/// project is named, one of whose projects was offered, answered, or moved.
/// It carries no roles: the console re-reads `/me`, which is where
/// who-owns-what comes from.
const OwnershipRefSchema = z.object({
  organizationId: z.string(),
  projectId: z.string().optional(),
});

export const RealtimeEventSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("ownership:changed"),
    data: OwnershipRefSchema,
  }),
  z.object({
    type: z.literal("invite:created"),
    data: IncomingInviteSchema,
  }),
  z.object({
    type: z.literal("invite:sent"),
    data: InviteRefSchema,
  }),
  z.object({
    type: z.literal("invite:revoked"),
    data: InviteRefSchema,
  }),
  z.object({
    type: z.literal("invite:resolved"),
    data: InviteRefSchema,
  }),
]);

export type RealtimeEvent = z.infer<typeof RealtimeEventSchema>;

export const RealtimeEventDataSchema = {
  "ownership:changed": OwnershipRefSchema,
  "invite:created": IncomingInviteSchema,
  "invite:sent": InviteRefSchema,
  "invite:revoked": InviteRefSchema,
  "invite:resolved": InviteRefSchema,
} as const;
