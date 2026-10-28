import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem } from "@/lib/api/fetch";

// Each field read alone: one auth spells otherwise leaves the rest.
const text = z.string().optional().catch(undefined);

/** What auth answers an invitation sent: what the live notices need, and the
 *  link when it hands one back. */
export const InviteSentSchema = z.object({
  id: text,
  link: text,
  expiresAt: text,
  ownerOrganizationId: text,
});

/** A roster's change: whose console to tell, and the organization's. */
export const RosterChangeSchema = z.object({ email: text, ownerOrganizationId: text });

/** An offer's change: who holds it now, and whose it replaced. */
export const OfferChangeSchema = z.object({ offeredToEmail: text, withdrawnFromEmail: text });

/**
 * Standard upstream error unwrapper for member management actions.
 */
export async function answerMemberAction(res: Response | null): Promise<ActionResult> {
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);
  return { ok: true };
}
