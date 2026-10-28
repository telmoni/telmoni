import { z } from "zod";

import { InviteScope, Role } from "@/lib/types/enums";

export const IncomingInviteSchema = z.object({
  id:                 z.string(),
  // The two ladders spell their roles the same, so the level is this, never
  // the role.
  scope:              z.enum(InviteScope),
  targetId:           z.string(),
  targetName:         z.string(),
  role:               z.enum(Role),
  // `null` once the person who sent it has deleted their account: the
  // invitation outlives them, and is still the organization's to honour.
  inviterEmail:       z.string().nullable(),
  inviterDisplayName: z.string().nullable().optional(),
  expiresAt:          z.string(),
  createdAt:          z.string(),
});
export type IncomingInvite = z.infer<typeof IncomingInviteSchema>;
