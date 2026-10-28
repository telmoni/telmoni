"use server";

import { type ActionResult, refused } from "@/lib/action-result";
import { admitOrganization } from "@/lib/server/admit";
import { organizationHeaders } from "@/lib/server/entities/identity-context";
import { NOT_DRAWN, NewestDrawn, markRead } from "@/lib/server/newest-drawn";

/** The organization's own feed read up to `before`, the newest notice the
 *  overview drew: the organization it rendered, which the action refuses to
 *  mistake for another, and nothing that landed since. */
export async function markOrganizationReadAction(
  organizationId: string,
  before: string,
): Promise<ActionResult> {
  // Posted, like the id: a time the page could not have drawn asks nothing.
  if (!NewestDrawn.safeParse(before).success) return refused(NOT_DRAWN);
  const gate = await admitOrganization({ call: "notifications:read", limit: 20 }, organizationId);
  if (!gate.ok) return refused(gate.error);
  return markRead(organizationHeaders(gate.ctx), before);
}
