"use server";

import { revalidatePath } from "next/cache";

import { type ActionResult, refused } from "@/lib/action-result";
import { extractProblem, tryFetchWithTimeout } from "@/lib/api/fetch";
import { env } from "@/lib/env";
import { admitProject } from "@/lib/server/admit";

/// How many days the project keeps its spans and the content kept with them:
/// one up to the longest sold, which the server holds, and the owner's or an
/// admin's alone to set, which it holds too.
export async function setRetentionAction(
  projectId: string,
  days: number,
): Promise<ActionResult> {
  const gate = await admitProject({ call: "project:retention", limit: 10 }, projectId);
  if (!gate.ok) return refused(gate.error);
  if (!Number.isInteger(days) || days < 1) return refused("Choose a whole number of days, one or more.");

  const res = await tryFetchWithTimeout(`${env.SERVER_URL}/internal/v1/telemetry/retention`, {
    method: "PUT",
    headers: { ...gate.headers, "content-type": "application/json" },
    body: JSON.stringify({ retention_days: days }),
  });
  if (!res) return refused("Failed to change the retention. Please try again.");
  if (!res.ok) return refused((await extractProblem(res)).message);

  revalidatePath("/(app)/[organization]/[project]/privacy", "page");
  return { ok: true };
}
