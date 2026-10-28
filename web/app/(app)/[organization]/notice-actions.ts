"use server";

import { refresh } from "next/cache";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { admitOrganization } from "@/lib/server/admit";
import { organizationHeaders } from "@/lib/server/entities/identity-context";
import { NOT_DRAWN, NewestDrawn } from "@/lib/server/newest-drawn";

/// Mark the organization's own feed read up to `before`, the newest notice
/// the overview drew: the organization it rendered, which the action refuses
/// to mistake for another, and nothing that landed since.
export async function markOrganizationReadAction(
  organizationId: string,
  before: string,
): Promise<ActionResult> {
  // Posted, like the id: a time the page could not have drawn asks nothing.
  if (!NewestDrawn.safeParse(before).success) return refused(NOT_DRAWN);
  const gate = await admitOrganization({ call: "notifications:read", limit: 20 }, organizationId);
  if (!gate.ok) return refused(gate.error);

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/notifications/read`, {
    method: "POST",
    headers: { ...organizationHeaders(gate.ctx), "content-type": "application/json" },
    body: JSON.stringify({ before }),
  });
  if (!res) {
    return refused("Notifications are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);
  refresh();
  return { ok: true };
}
