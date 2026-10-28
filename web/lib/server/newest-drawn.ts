import { refresh } from "next/cache";
import { z } from "zod";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";

/** The newest notice a page drew, by its `created_at`, as a Server Action
 *  takes it: an ISO time with its offset, passed on as it came, since the
 *  service compares it with the microseconds the row carries. */
export const NewestDrawn = z.iso.datetime({ offset: true });

export const NOT_DRAWN = "Couldn't tell what the page showed — reload and try again.";

/** The feed read up to `before`, the newest notice the page drew, for whoever
 *  `headers` name: every unread notice to there, as the service marks a feed,
 *  and none that arrived since. */
export async function markRead(
  headers: Record<string, string>,
  before: string,
): Promise<ActionResult> {
  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/notifications/read`, {
    method: "POST",
    headers: { ...headers, "content-type": "application/json" },
    body: JSON.stringify({ before }),
  });
  if (!res) {
    return refused("Notifications are unavailable right now — try again shortly.");
  }
  if (!res.ok) return refused((await extractProblem(res)).message);
  refresh();
  return { ok: true };
}
