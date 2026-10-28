"use server";

import { refresh } from "next/cache";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { admitProject } from "@/lib/server/admit";
import { NOT_DRAWN, NewestDrawn } from "@/lib/server/newest-drawn";

/// Mark the project's feed read up to `before`, the newest notice the page
/// drew: every unread notice to there, for everybody on the project, as the
/// service marks a feed, and none that arrived since. The project is the one
/// the overview rendered, by id, and the service answers whether the person
/// is on it, so a stale page marks no project it is not showing.
export async function markProjectReadAction(
  projectId: string,
  before: string,
): Promise<ActionResult> {
  // Posted, like the id: a time the page could not have drawn asks nothing.
  if (!NewestDrawn.safeParse(before).success) return refused(NOT_DRAWN);
  const gate = await admitProject({ call: "notifications:read", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/notifications/read`, {
    method: "POST",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify({ before }),
  });
  if (!res) {
    return refused("Notifications are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);
  refresh();
  return { ok: true };
}
