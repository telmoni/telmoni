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
 * The roles an invitation or a role change may hand out. A Server Action takes
 * whatever is posted to it, whatever its parameter's type says, so the role
 * is checked here before it is sent anywhere; auth refuses anything else too.
 */
export function isGivenRole(role: unknown): role is "admin" | "member" {
  return role === "admin" || role === "member";
}

export const NOT_A_GIVEN_ROLE = "Choose admin or member.";

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
