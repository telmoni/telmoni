import { z } from "zod";

/** What auth answers an invitation accepted or declined: who to tell, and
 *  which organization's console. */
export const SettledInviteSchema = z.object({
  inviteId: z.string().optional(),
  inviterEmail: z.email().nullish(),
  ownerOrganizationId: z.string().optional(),
});
export type SettledInvite = z.infer<typeof SettledInviteSchema>;
