import { z } from "zod";

export const IncomingInviteSchema = z.object({
  id:                 z.string(),
  scope:              z.string(),
  targetId:           z.string(),
  targetName:         z.string(),
  role:               z.string(),
  // `null` once the person who sent it has deleted their account: the
  // invitation outlives them, and is still the organization's to honour.
  inviterEmail:       z.string().nullable(),
  inviterDisplayName: z.string().nullable().optional(),
  expiresAt:          z.string(),
  createdAt:          z.string(),
});
export type IncomingInvite = z.infer<typeof IncomingInviteSchema>;
