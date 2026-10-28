"use server";

import { type ActionResult, refused } from "@/lib/action-result";
import { admitProject } from "@/lib/server/admit";
import { NOT_DRAWN, NewestDrawn, markRead } from "@/lib/server/newest-drawn";

/** The project's feed read up to `before`, the newest notice the page drew,
 *  for everybody on the project. The project is the one the overview
 *  rendered, by id, and the service answers whether the person is on it, so a
 *  stale page marks no project it is not showing. */
export async function markProjectReadAction(
  projectId: string,
  before: string,
): Promise<ActionResult> {
  // Posted, like the id: a time the page could not have drawn asks nothing.
  if (!NewestDrawn.safeParse(before).success) return refused(NOT_DRAWN);
  const gate = await admitProject({ call: "notifications:read", limit: 20 }, projectId);
  if (!gate.ok) return refused(gate.error);
  return markRead(gate.headers, before);
}
