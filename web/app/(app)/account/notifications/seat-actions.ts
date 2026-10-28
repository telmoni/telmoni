"use server";

import { refresh } from "next/cache";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, readBody, tryFetchWithTimeout } from "@/lib/api/fetch";
import { isProjectId } from "@/lib/ids";
import { env } from "@/lib/env";
import { publishEvent, userChannel } from "@/lib/events/publisher";
import { UNRESOLVED_PROJECT, admit } from "@/lib/server/admit";
import { personHeaders } from "@/lib/server/entities/identity-context";
import { SeatSeenSchema } from "@/lib/server/entities/organization";

// The person has seen the seat given to them — opened the project from the
// bell, or dismissed the row — so the bell stops showing it, in their other
// consoles too, told on their own channel. The seat stays.
export async function markSeatSeenAction(projectId: string): Promise<ActionResult> {
  const gate = await admit({ call: "seats:seen", limit: 30 });
  if (!gate.ok) return refused(gate.error);
  const { session } = gate;
  // The id becomes a path segment of the upstream URL: anything but a project
  // id is refused before it can aim the request elsewhere.
  if (!isProjectId(projectId)) return refused(UNRESOLVED_PROJECT);
  const res = await tryFetchWithTimeout(
    `${env.SERVER_URL}/internal/v1/me/seats/${encodeURIComponent(projectId)}/seen`,
    { method: "POST", headers: personHeaders(session) },
  );
  if (!res) {
    return refused("Organizations are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);
  const organizationId = (await readBody(res, SeatSeenSchema))
    ?.organizationId;
  if (organizationId) {
    await publishEvent(userChannel(session.email), {
      type: "membership:changed",
      data: { organizationId, projectId },
    });
  }
  refresh();
  return { ok: true };
}
