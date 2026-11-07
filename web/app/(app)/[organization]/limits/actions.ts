"use server";

import { revalidatePath } from "next/cache";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { isProjectId } from "@/lib/ids";
import { admitOrganization } from "@/lib/server/admit";
import { sessionHeaders } from "@/lib/server/entities/identity-context";
import type { Caps } from "@/lib/server/entities/limits";

const CAPS = [
  "spans",
  "content_bytes",
  "alerts",
  "monitors",
  "objectives",
  "alert_deliveries_per_day",
] as const;

/** One project's caps on its organization's pool, set whole by the
 *  organization's owners and admins; `null` shares the pool whole. */
export async function setProjectCapsAction(
  organizationId: string,
  projectId: string,
  caps: Caps,
): Promise<ActionResult> {
  const gate = await admitOrganization({ call: "organization:limits", limit: 20 }, organizationId, {
    role: "admin",
    refused: "Only the organization's owner or an admin can set a project's caps.",
  });
  if (!gate.ok) return refused(gate.error);
  // The id becomes a header of the lane's request, and a Server Action takes
  // whatever is posted to it.
  if (!isProjectId(projectId)) return refused("That project could not be found.");
  const body: Record<string, number | null> = {};
  for (const key of CAPS) {
    const cap = caps[key];
    if (cap !== null && !(Number.isSafeInteger(cap) && cap >= 0)) {
      return refused("A cap is a whole number, none or more; leave it empty to share the pool.");
    }
    body[key] = cap;
  }

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/limits`, {
    method: "PUT",
    headers: {
      ...sessionHeaders(gate.session, gate.ctx.organizationId, projectId),
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
  });
  if (!res) return refused("Failed to set the caps. Please try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);

  revalidatePath("/(app)/[organization]/limits", "page");
  return { ok: true };
}
